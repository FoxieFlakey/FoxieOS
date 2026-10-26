use std::sync::Arc;

use anyhow::Context;
use log::{info, warn};
use nice_drm_uwu::primary_gpu::{PrimaryGpu, master_state::MasterGuard};
use yoke::Yoke;

pub struct GraphicsServer {
    #[expect(unused)]
    gpu: Arc<PrimaryGpu>,
    #[expect(unused)]
    master_lock: Yoke<MasterGuard<'static>, Arc<PrimaryGpu>>,
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
        for (index, connector) in gpu
            .get_connectors(true)
            .context("Cannot retrieve all connectors")?
            .iter()
            .enumerate()
        {
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

        Ok(Self { gpu, master_lock })
    }
}
