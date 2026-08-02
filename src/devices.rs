use gst::prelude::*;
use gstreamer as gst;
use gtk::glib;

/// Assertion failures GStreamer's ALSA device provider prints while probing,
/// which say nothing a user can act on.
///
/// A machine with an audio output ALSA cannot describe produces one of these
/// per probe, on every launch. A Raspberry Pi does it by simply having a
/// second HDMI port with no display attached: the provider probes the port,
/// gets no caps back, and asserts. The device list is unaffected - the port
/// has no audio output to offer in the first place.
///
/// The assertions are inside gst-plugins-base, so the only thing we control is
/// whether they reach the terminal. Ignoring the provider entirely is not an
/// option: on a system with no PulseAudio or PipeWire it is the one supplying
/// every device we list.
const ALSA_PROBE_NOISE: [&str; 2] = ["gst_alsa_device_new", "gst_caps_append"];

/// List available audio *output* devices (cross-platform: PipeWire/Pulse
/// sinks on Linux, WASAPI endpoints on Windows) via GStreamer's own
/// DeviceMonitor, instead of shelling out to a platform-specific tool like
/// `pactl`.
/// Just the names, for printing. Keeps GStreamer's traits in here rather than
/// making the caller import a prelude to ask a simple question.
pub fn output_device_names() -> Result<Vec<String>, String> {
    Ok(list_audio_output_devices()?
        .iter()
        .map(|device| device.display_name().to_string())
        .collect())
}

pub fn list_audio_output_devices() -> Result<Vec<gst::Device>, String> {
    let monitor = gst::DeviceMonitor::new();
    let caps = gst::Caps::new_any();
    monitor
        .add_filter(Some("Audio/Sink"), Some(&caps))
        .ok_or("Failed to add device monitor filter")?;

    // Only around the probe itself, and only for those two messages, so
    // anything else GStreamer has to say still comes through.
    glib::log_set_default_handler(|domain, level, message| {
        if ALSA_PROBE_NOISE.iter().any(|noise| message.contains(noise)) {
            return;
        }
        glib::log_default_handler(domain, level, Some(message));
    });

    let started = monitor.start();
    let devices = started.is_ok().then(|| monitor.devices());
    monitor.stop();

    glib::log_unset_default_handler();

    started.map_err(|e| format!("Failed to start device monitor: {e}"))?;

    Ok(devices.unwrap_or_default().into_iter().collect())
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
                 renamed, or is otherwise unavailable. Choose an output on the main screen."
            )
        })
}
