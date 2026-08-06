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

/// Just the names, for printing. Keeps GStreamer's traits in here rather than
/// making the caller import a prelude to ask a simple question.
pub fn output_device_names() -> Result<Vec<String>, String> {
    Ok(list_audio_output_devices()?
        .iter()
        .map(|device| device.display_name().to_string())
        .collect())
}

/// Available audio *output* devices, through GStreamer's own DeviceMonitor
/// rather than a platform-specific tool like `pactl` - PipeWire and Pulse
/// sinks on Linux, WASAPI endpoints on Windows, from one call.
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

    Ok(devices
        .unwrap_or_default()
        .into_iter()
        .filter(plays_here)
        .collect())
}

/// Whether a device's sink is one this pipeline can actually use.
///
/// `gstreamer1.0-pipewire` adds a second device provider, so every output is
/// then reported twice - once building a `pulsesink`, once a `pipewiresink` -
/// under identical display names. TinePlayer has only ever depended on
/// `gstreamer1.0-pulseaudio | gstreamer1.0-alsa`, so the PipeWire provider
/// arrives uninvited, pulled in by a desktop environment rather than chosen.
///
/// It cannot simply be tolerated. Devices are matched by display name, so with
/// duplicates present the one taken is whichever the monitor happened to list
/// first - and measured on Debian 12 that is PipeWire's, which plays silence:
/// the pipeline builds without complaint, reports no error, and no audio ever
/// reaches the device. The same element works perfectly in a standalone
/// `gst-launch` pipeline, with stereo and 5.1, so what silences it here is not
/// yet understood. It is not the forced clock and not `async`; both were ruled
/// out by measurement on 2026-08-06.
///
/// Filtering on the element rather than deduplicating by name also means a
/// device that appears *only* from PipeWire is never offered, which is right
/// for the same reason: it would play nothing.
///
/// Excluding what does not work, rather than demanding `pulsesink`, is what
/// keeps a machine with no sound server working - there the devices come from
/// `alsasink`, and insisting on Pulse would leave the list empty.
fn plays_here(device: &gst::Device) -> bool {
    let factory = device
        .create_element(None)
        .ok()
        .and_then(|element| element.factory().map(|f| f.name().to_string()));
    // A device that cannot build an element at all is kept, so the failure is
    // reported where it can name the device rather than vanishing from a list.
    factory.is_none_or(|name| name != "pipewiresink")
}

/// What to send audio to when nobody has chosen yet.
///
/// The system's own default where the platform says which that is, and
/// otherwise the first device offered. Both are guesses, and a guess is worth
/// making here: an unset primary output means a first run that opens the menu
/// and cannot play anything from it, which reads as broken rather than as
/// unconfigured.
///
/// The property naming the default differs by provider and none of them are
/// guaranteed to be there, so all three spellings are tried and the answer is
/// allowed to be "none of these said". PulseAudio and PipeWire use
/// `is-default`; WASAPI and Core Audio have been seen with `default` and
/// `device.default`.
pub fn default_output_device_name() -> Option<String> {
    let devices = list_audio_output_devices().ok()?;
    devices
        .iter()
        .find(|device| {
            device.properties().is_some_and(|properties| {
                ["is-default", "default", "device.default"]
                    .iter()
                    .any(|key| properties.get::<bool>(*key).unwrap_or(false))
            })
        })
        .or_else(|| devices.first())
        .map(|device| device.display_name().to_string())
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
