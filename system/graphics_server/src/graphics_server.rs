use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
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
use yoke::Yoke;

pub struct GraphicsServer {
    #[expect(unused)]
    gpu: Arc<PrimaryGpu>,
    #[expect(unused)]
    master_lock: Yoke<MasterGuard<'static>, Arc<PrimaryGpu>>,
    #[expect(unused)]
    main_crtc: Arc<Crtc>,
    #[expect(unused)]
    main_encoder: Arc<Encoder>,
    #[expect(unused)]
    main_connector: Connector,
    #[expect(unused)]
    main_mode: Mode,
    #[expect(unused)]
    main_plane: Arc<Plane>,
    #[expect(unused)]
    framebuffer_handle: Framebuffer,
    #[expect(unused)]
    framebuffer: DumbBuffer,
    #[expect(unused)]
    framebuffer_mapping: DumbMapping<'static>,
}

impl GraphicsServer {
    pub fn new() -> anyhow::Result<Self> {
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

        let buf = gpu
            .alloc_dumb_buffer(
                (
                    main_mode.resolution().0.into(),
                    main_mode.resolution().1.into(),
                ),
                BufferModifier::Linear,
                BufferFormat::Xrgb8888,
            )
            .context("Cannot create dumb buffer for framebuffer")?;

        let framebuffer = buf
            .create_framebuffer()
            .context("Cannot create framebuffer out of dumb buffer")?;

        gpu.create_atomic_req()
            // Connect CRTC to connector
            .connector_attach_crtc(&main_connector, Some(&main_crtc))
            // Set mode on CRTC
            .crtc_set_mode(&main_crtc, main_mode.clone())
            .crtc_set_active(&main_crtc, true)
            // Then attach CRTC and framebuffer to a plane
            .plane_attach_crtc(&main_plane, Some(&main_crtc))
            .plane_attach_framebuffer(&main_plane, Some(&framebuffer))
            // And now tell CRTC where to scan
            .plane_set_crtc_x(&main_plane, 0)
            .plane_set_crtc_y(&main_plane, 0)
            .plane_set_crtc_w(&main_plane, main_mode.resolution().0.into())
            .plane_set_crtc_h(&main_plane, main_mode.resolution().1.into())
            // And tell where to place the framebuffer in a plane
            .plane_set_src_x(&main_plane, 0)
            .plane_set_src_y(&main_plane, 0)
            .plane_set_src_w(&main_plane, framebuffer.width())
            .plane_set_src_h(&main_plane, framebuffer.height())
            // And do first pageflip too
            .pageflip(true)
            .commit()
            .context("Cannot do atomic modeset")?;

        info!("Perform atomic set!");

        let mut framebuffer_mapping = buf
            .map()
            .context("Cannot map framebuffer")?
            .forget_lifetime();

        // SAFETY: We havent sent that buffer to other process yet
        let bytes = unsafe { framebuffer_mapping.get_mut() };
        let color = (0x682B2Au32).to_le_bytes();
        for chunk in bytes.as_chunks_mut().0 {
            *chunk = color;
        }

        framebuffer.dirty().context("Cannot dirty framebuffer")?;

        Ok(Self {
            gpu,
            master_lock,
            main_crtc,
            main_encoder,
            main_connector,
            main_mode,
            main_plane,
            framebuffer_mapping,
            framebuffer: buf,
            framebuffer_handle: framebuffer,
        })
    }
}
