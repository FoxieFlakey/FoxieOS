use anyhow::Context;
use vulkano::{
    VulkanLibrary,
    instance::{Instance, InstanceCreateInfo},
};

fn main() -> anyhow::Result<()> {
    let vk_lib = VulkanLibrary::new().context("Cannot find vulkan loader")?;
    let instance = Instance::new(
        vk_lib.clone(),
        InstanceCreateInfo {
            application_name: Some("Meow Meow test".to_string()),
            ..Default::default()
        },
    )
    .context("Cannot create vulkan instance")?;

    for (idx, dev) in instance
        .enumerate_physical_devices()
        .context("Cannot find GPUs")?
        .enumerate()
    {
        let vk_version = dev.api_version();
        let props = dev.properties();
        println!("{idx:2}. Bus ID: {:?}", props.pci_bus);
        println!("    Vulkan version: {vk_version}");
        println!("    Device name: {}", props.device_name);
        println!("    Driver name: {:?}", props.driver_name);
        println!("    Driver info: {:?}", props.driver_info);
        println!("    Driver version: {}", props.driver_version);
        println!("    Device type: {:?}", props.device_type);
        if props.device_luid_valid.unwrap_or(false) {
            println!("    Device LUID: {:?}", props.device_luid);
        } else {
            println!("    Device LUID (invalid): {:?}", props.device_luid);
        }
        println!("    Driver UUID: {:?}", props.driver_uuid);
    }

    Ok(())
}
