use std::{
    mem,
    os::fd::AsFd,
    sync::{Arc, OnceLock},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use foxie_base::util::retry_eintr;
use log::{info, warn};
use nice_drm_uwu::{
    gpu::ClientCapability,
    primary_gpu::{
        PrimaryGpu,
        connector::{Connector, State},
        crtc::Crtc,
        dumb_buffer::{BufferFormat, BufferModifier, DumbBuffer, mapping::DumbMapping},
        encoder::Encoder,
        framebuffer::Framebuffer,
        master_state::MasterGuard,
        mode::Mode,
        plane::Plane,
    },
};
use nix::{
    poll::{self, PollFd, PollFlags, PollTimeout},
    sys::{
        eventfd::EventFd,
        timer::{Expiration, TimerSetTimeFlags},
        timerfd::{ClockId, TimerFd, TimerFlags},
    },
};
use yoke::Yoke;

pub struct GraphicsServer {
    gpu: Arc<PrimaryGpu>,
    #[expect(unused)]
    master_lock: Yoke<MasterGuard<'static>, Arc<PrimaryGpu>>,
    main_crtc: Arc<Crtc>,
    #[expect(unused)]
    main_encoder: Arc<Encoder>,
    main_connector: Connector,
    main_mode: Mode,
    main_plane: Arc<Plane>,

    do_shutdown: Arc<EventFd>,
    thread: OnceLock<JoinHandle<()>>,
}

pub(super) struct Frame {
    framebuffer: Framebuffer,
    #[expect(unused)]
    buf: DumbBuffer,
    mapping: DumbMapping<'static>,
}

impl Drop for GraphicsServer {
    fn drop(&mut self) {
        self.do_shutdown
            .write(1)
            .expect("Cannot notify graphics server thread");

        let join = self.thread.take().unwrap();
        if join.thread().id() != thread::current().id() {
            join.join().unwrap();
        }
    }
}

impl GraphicsServer {
    pub fn new() -> anyhow::Result<Arc<Self>> {
        let gpu = PrimaryGpu::open("/dev/dri/card0").context("Cannot open GPU")?;
        let busid = gpu.get_basic_gpu().get_bus_id();
        let driver = gpu.get_basic_gpu().driver_info();
        info!(
            "graphics_server: Selected GPU {:?} (driver {:?}) on {:?}",
            driver.description, driver.name, busid
        );

        let master_lock = Yoke::try_attach_to_cart(gpu.clone(), |x| x.lock_master())
            .context("Cannot acquire master lock on main GPU")?;

        let basic_gpu = gpu.get_basic_gpu();
        if !basic_gpu
            .is_dumb_buffer_supported()
            .context("Cannot query dumb buffer support")?
        {
            bail!("GPU lacks required driver support: dumb buffer");
        }

        if !basic_gpu
            .is_monotonic_timestamp_supported()
            .context("Cannot query monotonic timestamp support")?
        {
            bail!("GPU lacks required driver support: monotonic timestamp");
        }

        if !basic_gpu
            .is_prime_handle_supported()
            .context("Cannot query prime handle support for import export")?
        {
            bail!("GPU lacks required driver support: prime handle");
        }

        if !basic_gpu
            .is_timeline_syncobj_supported()
            .context("Cannot query timeline syncobj support")?
        {
            bail!("GPU lacks required driver support: timeline syncobj");
        }

        basic_gpu
            .set_client_capability(ClientCapability::Atomic, true)
            .context("Cant enable atomic modesetting on GPU")?;

        basic_gpu
            .set_client_capability(ClientCapability::UniversalPlanes, true)
            .context("Cant enable universal planes on GPU")?;

        info!("Selected GPU is eligible for graphics_server usage");
        info!("CRTCs detected:");
        for (index, crtc) in gpu.get_crtcs().iter().enumerate() {
            match crtc.mode() {
                Ok(Some(x)) => {
                    info!("{index:2}. {crtc} => Mode is {x}");
                }
                Ok(None) => {
                    info!("{index:2}. {crtc} => Inactive");
                }
                Err(e) => {
                    warn!("{index:2}. Cannot get {crtc} mode, skipping: {e}");
                }
            }
        }

        info!("Encoders detected:");
        for (index, encoder) in gpu.get_encoders().iter().enumerate() {
            info!("{index:2}. {encoder}");
        }

        info!("Connectors detected:");
        let connectors = gpu
            .get_connectors(true)
            .context("Cannot retrieve all connectors")?;
        for (index, connector) in connectors.iter().enumerate() {
            info!("{index:2}. {connector}");
            match connector.current_encoder() {
                Some(encoder) => {
                    info!("    Attached encoder: {}", encoder);
                    match encoder.get_crtc() {
                        Ok(Some(x)) => {
                            info!("    CRTC: {}", x);
                            match x.mode() {
                                Ok(Some(mode)) => {
                                    info!("    Mode: {mode}");
                                }
                                Ok(None) => info!("    Mode: None"),
                                Err(e) => {
                                    warn!("Cannot get CRTC information: {e}")
                                }
                            }
                        }
                        Ok(None) => {
                            info!("    CRTC: Not attached");
                        }
                        Err(e) => warn!("Cannot get current active CRTC: {e}"),
                    }
                }
                None => info!("    No active encoder"),
            }
        }

        let (main_connector, main_encoder, main_crtc, main_mode, main_plane) = connectors
            .into_iter()
            .filter(|x| x.state() == State::Connected)
            .map(|x| (x.current_encoder().cloned(), x))
            .flat_map(|(encoder, connector)| encoder.map(|x| (connector, x)))
            .flat_map(|(conn, enc)| match enc.get_crtc() {
                Ok(crtc) => crtc.map(|x| (conn, enc, x)),
                Err(e) => {
                    warn!("Cannot get CRTC info that attached to encoder {enc} (thats attached to connector {conn}), skipping this config: {e}");
                    None
                }
            })
            .flat_map(|(conn, enc, crtc)| {
                match crtc.mode() {
                    Ok(mode) => mode.map(|mode| (conn, enc, crtc, mode)),
                    Err(e) => {
                        warn!("Cannot get mode of CRTC {crtc} that is attached to encoder {enc} (thats attached to connector {conn}), skipping this config: {e}");
                        None
                    },
                }
            })
            .flat_map(|(conn, enc, crtc, mode)| {
                match crtc.get_planes().first() {
                    Some(plane) => {
                        let plane = plane.clone();
                        Some((conn, enc, crtc, mode, plane))
                    },
                    None => {
                        warn!("Cannot get primary plane of CRTC {crtc} that is attached to encoder {enc} (thats attached to connector {conn}), skipping this config because missing plane");
                        None
                    },
                }
            })
            .next()
            .ok_or(anyhow!("Cannot find active usable connector"))?;

        info!("Found this config ({main_crtc} -> {main_encoder} -> {main_connector}) using it");

        gpu.create_atomic_req()
            // Connect CRTC to connector
            .connector_attach_crtc(&main_connector, Some(&main_crtc))
            // Set mode on CRTC
            .crtc_set_mode(&main_crtc, main_mode.clone())
            .crtc_set_active(&main_crtc, true)
            // Then dont attach CRTC or framebuffer to plane
            .plane_attach_crtc(&main_plane, None)
            .plane_attach_framebuffer(&main_plane, None)
            .commit()
            .context("Cannot do atomic modeset")?;

        info!("Perform atomic set!");
        let this = Arc::new(Self {
            gpu,
            master_lock,
            main_crtc,
            main_encoder,
            main_connector,
            main_mode,
            main_plane,
            do_shutdown: Arc::new(EventFd::new().context("Cannot create shutdown eventfd")?),
            thread: OnceLock::new(),
        });

        let this_cloned = this.clone();
        this.thread
            .set(thread::spawn(move || this_cloned.run()))
            .unwrap();
        Ok(this)
    }

    fn alloc_framebuffer(&self) -> anyhow::Result<Frame> {
        let dumb_buf = self
            .gpu
            .alloc_dumb_buffer(
                (
                    self.main_mode.resolution().0.into(),
                    self.main_mode.resolution().1.into(),
                ),
                BufferModifier::Linear,
                BufferFormat::Xrgb8888,
            )
            .context("Cannot allocate dumb buffer")?;
        let framebuffer = dumb_buf
            .create_framebuffer()
            .context("Cannot crate framebuffer handle from dumb buffer")?;
        let mapping = dumb_buf
            .map()
            .context("Cannot map dumb buffer")?
            .forget_lifetime();

        Ok(Frame {
            framebuffer,
            buf: dumb_buf,
            mapping,
        })
    }

    fn run(mut self: Arc<Self>) {
        let shutdown_fd = self.do_shutdown.clone();
        let server = Arc::downgrade(&self);

        let mut front_buffer = self
            .alloc_framebuffer()
            .expect("Cannot allocate front buffer");
        let mut back_buffer = self
            .alloc_framebuffer()
            .expect("Cannot allocate back buffer");

        // Do first modeset
        self.gpu
            .create_atomic_req()
            // Connect CRTC to connector
            .connector_attach_crtc(&self.main_connector, Some(&self.main_crtc))
            // Set mode on CRTC
            .crtc_set_mode(&self.main_crtc, self.main_mode.clone())
            .crtc_set_active(&self.main_crtc, true)
            // Then attach CRTC and framebuffer to a plane
            .plane_attach_crtc(&self.main_plane, Some(&self.main_crtc))
            .plane_attach_framebuffer(&self.main_plane, Some(&front_buffer.framebuffer))
            // And now tell CRTC where to scan
            .plane_set_crtc_x(&self.main_plane, 0)
            .plane_set_crtc_y(&self.main_plane, 0)
            .plane_set_crtc_w(&self.main_plane, self.main_mode.resolution().0.into())
            .plane_set_crtc_h(&self.main_plane, self.main_mode.resolution().1.into())
            // And tell where to place the framebuffer in a plane
            .plane_set_src_x(&self.main_plane, 0)
            .plane_set_src_y(&self.main_plane, 0)
            .plane_set_src_w(&self.main_plane, front_buffer.framebuffer.width())
            .plane_set_src_h(&self.main_plane, front_buffer.framebuffer.height())
            // And do first pageflip too
            .pageflip(true)
            .commit()
            .expect("Cannot do atomic modeset");

        drop(self);

        let render_timer = TimerFd::new(
            ClockId::CLOCK_MONOTONIC,
            TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
        )
        .expect("Cannot create render timer");
        render_timer
            .set(
                Expiration::Interval(Duration::from_secs(1).into()),
                TimerSetTimeFlags::empty(),
            )
            .expect("Cannot arm timer");

        let colors: [u32; _] = [0x682b2a, 0xf9a147, 0x1862ff, 0xfcfd4f, 0xff74b3];
        let mut cur_seq = 0;

        loop {
            let mut polls = [
                PollFd::new(shutdown_fd.as_fd(), PollFlags::POLLIN),
                PollFd::new(render_timer.as_fd(), PollFlags::POLLIN),
            ];

            retry_eintr(|| poll::poll(&mut polls, PollTimeout::NONE)).expect("Cannot poll");

            if !polls[0].revents().unwrap().is_empty() {
                shutdown_fd.read().expect("Cannot acknowledge shutdown fd");
                break;
            }

            if !polls[1].revents().unwrap().is_empty() {
                render_timer.wait().expect("Cannot consume timer event");

                // Do page flip
                self = match server.upgrade() {
                    Some(x) => x,
                    None => continue,
                };

                mem::swap(&mut front_buffer, &mut back_buffer);

                front_buffer
                    .framebuffer
                    .dirty()
                    .expect("Cannot dirty front buffer");
                self.gpu
                    .create_atomic_req()
                    .plane_attach_framebuffer(&self.main_plane, Some(&front_buffer.framebuffer))
                    .pageflip(true)
                    .commit()
                    .expect("Cannot do page flip");

                // Then we can render to back buffer
                let color_to_fill = colors[cur_seq].to_le_bytes();

                // SAFETY: This is fine, the CRTC is not currently reading it nor anything
                // writing/reading to this
                let mapping = unsafe { back_buffer.mapping.get_mut() };
                for pixel in mapping.as_chunks_mut().0 {
                    *pixel = color_to_fill;
                }

                cur_seq = (cur_seq + 1) % colors.len();
            }
        }
    }
}
