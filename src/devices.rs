use gst::prelude::*;
use gstreamer as gst;

/// List available audio *output* devices (cross-platform: PipeWire/Pulse
/// sinks on Linux, WASAPI endpoints on Windows) via GStreamer's own
/// DeviceMonitor, instead of shelling out to a platform-specific tool like
/// `pactl`.
pub fn list_audio_output_devices() -> Result<Vec<gst::Device>, String> {
    let monitor = gst::DeviceMonitor::new();
    let caps = gst::Caps::new_any();
    monitor
        .add_filter(Some("Audio/Sink"), Some(&caps))
        .ok_or("Failed to add device monitor filter")?;

    monitor
        .start()
        .map_err(|e| format!("Failed to start device monitor: {e}"))?;
    let devices = monitor.devices();
    monitor.stop();

    Ok(devices.into_iter().collect())
}

/// Re-find a previously chosen device by its display name (what we persist
/// in the config file) on a later run.
pub fn find_audio_output_device(name: &str) -> Result<gst::Device, String> {
    let devices = list_audio_output_devices()?;
    devices
        .into_iter()
        .find(|d| d.display_name() == name)
        .ok_or_else(|| {
            format!(
                "Audio output device \"{name}\" not found. It may have been unplugged, \
                 renamed, or is otherwise unavailable — run --configure again."
            )
        })
}
