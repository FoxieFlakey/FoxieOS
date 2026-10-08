use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand};
use foxie_base::service_manager::{IServiceManager, iservicemanager};
use libbinder::{ContextManagerInfo, Runtime};
use libbinder_basic::TryFromProxy;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Shutdown,
    Ping,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let runtime = Runtime::new(
        foxie_base::MAIN_BINDER_PATH,
        ContextManagerInfo::Remote(Box::new(|x| Ok(<dyn IServiceManager>::try_from_proxy(x)?))),
    )
    .context("Cannot initialize binder runtime")?;

    let init = runtime
        .get_manager()
        .downcast_ref::<iservicemanager::ProxyIServiceManager>()
        .unwrap();
    let init = init
        .get_init()
        .context("Cannot call to service manager")?
        .map_err(|x| anyhow!("Cannot get init object: {}", x.0))?;

    match cli.command {
        Cmd::Ping => {
            let ret = &*init.ping().context("Cannot ping init :(")?;
            println!("Init said '{ret}'");
            Ok(())
        }
        Cmd::Shutdown => {
            init.request_shutdown()
                .context("Cannot call init for shutdown")?
                .map_err(|x| anyhow!("Cannot request shutdown: {}", x.0))?;
            println!("Requested shutdown!");
            Ok(())
        }
    }
}
