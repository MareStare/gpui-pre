//! The renderer makes the device after a lost device from the instance of the lost
//! device. wgpu compares devices by an id that each instance counts, so the new device
//! must differ from the lost one there.

use gpui_wgpu::{WgpuContext, wgpu};

fn request_device(adapter: &wgpu::Adapter) -> wgpu::Device {
    let (device, _queue) =
        gpui::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("requesting a device");
    device
}

#[test]
fn a_second_device_of_an_instance_differs_from_the_first() {
    let instance = WgpuContext::instance(None, wgpu::Backends::all());
    let adapter = [false, true]
        .into_iter()
        .find_map(|force_fallback_adapter| {
            let options = wgpu::RequestAdapterOptions {
                force_fallback_adapter,
                ..Default::default()
            };
            gpui::block_on(instance.request_adapter(&options)).ok()
        });
    let Some(adapter) = adapter else {
        eprintln!("The host has no adapter. The test checks nothing.");
        return;
    };

    let lost = request_device(&adapter);
    let recovered = request_device(&adapter);

    assert_ne!(lost, recovered);
}
