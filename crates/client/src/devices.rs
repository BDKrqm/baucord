// devices.rs — NEW FILE (backend for robust enumeration + open with fallbacks)

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, Host, HostId, SampleFormat, SampleRate, Stream, StreamConfig};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// One selectable “mode” under a device (e.g., Mono 1ch, Stereo 2ch).
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub id: String, // stable id = device_id + "::" + mode tag (e.g., "::in:1ch" / "::out:2ch")
    pub label: String, // what UI shows under the device (e.g., "Mono (1 ch)" / "Stereo (2 ch)")
    pub channels: u16, // 1 or 2 (we don’t expose >2 for voice)
    pub supports_48k: bool,
    pub is_input: bool,
}

/// Physical/logical audio device with grouped endpoints.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub id: String, // stable across runs (best-effort)
    pub name: String,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub inputs: Vec<Endpoint>,
    pub outputs: Vec<Endpoint>,
}

#[derive(Clone, Debug)]
pub struct DeviceInventory {
    pub devices: Vec<DeviceInfo>,
    pub default_input_id: Option<String>,
    pub default_output_id: Option<String>,
}

/// Public helpers your app / settings UI can call.
pub mod audio_devices {
    use super::*;

    /// Enumerate audio devices, preferring ALSA on Linux, and collapse into grouped DeviceInfo.
    pub fn enumerate() -> DeviceInventory {
        let hosts = preferred_hosts();

        // Collect per-device by stable key
        let mut map: HashMap<String, DeviceInfo> = HashMap::new();
        let mut default_in: Option<String> = None;
        let mut default_out: Option<String> = None;

        for host in hosts {
            // Defaults
            let d_in = host.default_input_device().as_ref().map(stable_id);
            let d_out = host.default_output_device().as_ref().map(stable_id);
            if default_in.is_none() {
                default_in = d_in.clone();
            }
            if default_out.is_none() {
                default_out = d_out.clone();
            }

            // Inputs
            if let Ok(iter) = host.input_devices() {
                for dev in iter {
                    if let Some(info) = describe_device(&host, &dev, true, d_in.as_deref()) {
                        upsert_device(&mut map, info);
                    }
                }
            }

            // Outputs
            if let Ok(iter) = host.output_devices() {
                for dev in iter {
                    if let Some(info) = describe_device(&host, &dev, false, d_out.as_deref()) {
                        upsert_device(&mut map, info);
                    }
                }
            }
        }

        // Deduplicate by id (prefer first seen)
        let mut seen = HashSet::new();
        let mut devices: Vec<DeviceInfo> = map
            .into_values()
            .filter(|d| seen.insert(d.id.clone()))
            .collect();

        // Sort for pretty UI
        devices.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        DeviceInventory {
            devices,
            default_input_id: default_in,
            default_output_id: default_out,
        }
    }

   pub fn open_input_selected<F>(
    endpoint_id_or_empty_for_default: &str,
    on_data_f32: F,
) -> Result<(cpal::Stream, cpal::StreamConfig, String), String>
where
    F: FnMut(&[f32], &cpal::InputCallbackInfo) + Send + 'static,
{
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{BufferSize, SampleRate, StreamConfig};
    use std::sync::{Arc, Mutex};

    let hosts = preferred_hosts();
    let (resolved_dev, resolved_channels, resolved_id) =
        resolve_endpoint(&hosts, endpoint_id_or_empty_for_default, true)
            .or_else(|| resolve_endpoint(&hosts, "", true))
            .ok_or_else(|| "No input device available".to_string())?;

    // Wrap the callback so it can be reused across attempts without moving F
    let cb = Arc::new(Mutex::new(on_data_f32));

    // Preferred attempts
    for cfg in preferred_input_cfgs(resolved_channels) {
        let cb_cl = cb.clone();
        if let Ok(stream) = resolved_dev.build_input_stream(
            &cfg,
            move |data: &[f32], info| {
                if let Ok(mut f) = cb_cl.lock() { (f)(data, info) }
            },
            move |err| log::warn!("Input stream error: {err}"),
            None,
        ) {
            return Ok((stream, cfg, resolved_id));
        }
    }

    // Fallback to first supported (near 48k)
    let fallback = first_supported_input_config(&resolved_dev)
        .ok_or_else(|| "Input device has no supported configs".to_string())?;
    let cb_cl = cb.clone();
    let stream = resolved_dev
        .build_input_stream(
            &fallback,
            move |data: &[f32], info| {
                if let Ok(mut f) = cb_cl.lock() { (f)(data, info) }
            },
            move |err| log::warn!("Input stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to open input: {e}"))?;

    Ok((stream, fallback, resolved_id))
} 

    pub fn open_output_selected<F>(
        endpoint_id_or_empty_for_default: &str,
        on_write_f32: F,
    ) -> Result<(cpal::Stream, cpal::StreamConfig, String), String>
    where
        F: FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + Clone + 'static,
    {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::{BufferSize, SampleRate, StreamConfig};

        let hosts = preferred_hosts();
        let (resolved_dev, resolved_channels, resolved_id) =
            resolve_endpoint(&hosts, endpoint_id_or_empty_for_default, false)
                .or_else(|| resolve_endpoint(&hosts, "", false))
                .ok_or_else(|| "No output device available".to_string())?;

        let try_cfgs = preferred_output_cfgs(resolved_channels);
        for cfg in try_cfgs {
            let mut cb = on_write_f32.clone();
            if let Ok(stream) = resolved_dev.build_output_stream(
                &cfg,
                move |data: &mut [f32], info| (cb)(data, info),
                move |err| log::warn!("Output stream error: {err}"),
                None,
            ) {
                return Ok((stream, cfg, resolved_id));
            }
        }

        let fallback = first_supported_output_config(&resolved_dev)
            .ok_or_else(|| "Output device has no supported configs".to_string())?;
        let mut cb = on_write_f32.clone();
        let stream = resolved_dev
            .build_output_stream(
                &fallback,
                move |data: &mut [f32], info| (cb)(data, info),
                move |err| log::warn!("Output stream error: {err}"),
                None,
            )
            .map_err(|e| format!("Failed to open output: {e}"))?;

        Ok((stream, fallback, resolved_id))
    }
}

// ───────────────────────────────── internal helpers ──────────────────────────

#[cfg(target_os = "linux")]
fn preferred_hosts() -> Vec<Host> {
    let mut v = Vec::new();
    if let Ok(h) = cpal::host_from_id(HostId::Alsa) {
        v.push(h);
    }
    v.push(cpal::default_host());
    v
}

#[cfg(not(target_os = "linux"))]
fn preferred_hosts() -> Vec<Host> {
    vec![cpal::default_host()]
}

fn stable_id(d: &cpal::Device) -> String {
    d.name().unwrap_or_else(|_| "Unknown".to_string())
}

fn describe_device(
    host: &Host,
    dev: &Device,
    is_input: bool,
    default_id: Option<&str>,
) -> Option<DeviceInfo> {
    let name = dev.name().ok()?;
    let id = stable_id(dev);

    let mut min_ch = u16::MAX;
    let mut max_ch = 0u16;
    let mut supports_48k = false;

    // Gather channel bounds + 48k support
    if is_input {
        let cfgs = dev.supported_input_configs().ok()?;
        for c in cfgs {
            min_ch = min_ch.min(c.channels());
            max_ch = max_ch.max(c.channels());
            let r = c.min_sample_rate().0..=c.max_sample_rate().0;
            if r.contains(&48_000) {
                supports_48k = true;
            }
        }
    } else {
        let cfgs = dev.supported_output_configs().ok()?;
        for c in cfgs {
            min_ch = min_ch.min(c.channels());
            max_ch = max_ch.max(c.channels());
            let r = c.min_sample_rate().0..=c.max_sample_rate().0;
            if r.contains(&48_000) {
                supports_48k = true;
            }
        }
    }

    if min_ch == u16::MAX || max_ch == 0 {
        // No configs
        return None;
    }

    // Build endpoints (we expose at most Mono and Stereo modes for voice)
    let mut inputs: Vec<Endpoint> = Vec::new();
    let mut outputs: Vec<Endpoint> = Vec::new();

    if is_input {
        if min_ch <= 1 {
            inputs.push(Endpoint {
                id: format!("{id}::in:1ch"),
                label: "Mono (1 ch)".to_string(),
                channels: 1,
                supports_48k,
                is_input: true,
            });
        }
        if max_ch >= 2 {
            inputs.push(Endpoint {
                id: format!("{id}::in:2ch"),
                label: "Stereo (2 ch)".to_string(),
                channels: 2,
                supports_48k,
                is_input: true,
            });
        }
    } else {
        if max_ch >= 2 {
            outputs.push(Endpoint {
                id: format!("{id}::out:2ch"),
                label: "Main L/R (2 ch)".to_string(),
                channels: 2,
                supports_48k,
                is_input: false,
            });
        }
        if min_ch <= 1 {
            outputs.push(Endpoint {
                id: format!("{id}::out:1ch"),
                label: "Mono (1 ch)".to_string(),
                channels: 1,
                supports_48k,
                is_input: false,
            });
        }
    }

    Some(DeviceInfo {
        id: id.clone(),
        name: name.clone(),
        is_default_input: default_id == Some(id.as_str()),
        is_default_output: default_id == Some(id.as_str()),
        inputs,
        outputs,
    })
}

fn upsert_device(map: &mut HashMap<String, DeviceInfo>, incoming: DeviceInfo) {
    match map.get_mut(&incoming.id) {
        Some(existing) => {
            existing.is_default_input |= incoming.is_default_input;
            existing.is_default_output |= incoming.is_default_output;
            if !incoming.inputs.is_empty() {
                merge_endpoints(&mut existing.inputs, incoming.inputs);
            }
            if !incoming.outputs.is_empty() {
                merge_endpoints(&mut existing.outputs, incoming.outputs);
            }
        }
        None => {
            map.insert(incoming.id.clone(), incoming);
        }
    }
}

fn merge_endpoints(dst: &mut Vec<Endpoint>, src: Vec<Endpoint>) {
    let mut seen = HashSet::<String>::new();
    for e in dst.iter() {
        seen.insert(e.id.clone());
    }
    for e in src {
        if seen.insert(e.id.clone()) {
            dst.push(e);
        }
    }
}

/// Convert “saved endpoint id” into (Device, channels, resolved endpoint id).
fn resolve_endpoint(hosts: &[Host], saved: &str, is_input: bool) -> Option<(Device, u16, String)> {
    // saved format: "" => default, or "<device_name>::(in|out):{1ch|2ch}"
    if saved.is_empty() {
        for h in hosts {
            let d = if is_input {
                h.default_input_device()
            } else {
                h.default_output_device()
            };
            if let Some(dev) = d {
                // pick best default endpoint: in→1ch first, out→2ch first
                let (ch, tag) = if is_input {
                    (1u16, "in:1ch")
                } else {
                    (2u16, "out:2ch")
                };
                let id = format!("{}::{}", stable_id(&dev), tag);
                return Some((dev, ch, id));
            }
        }
        return None;
    }

    // Parse device name and mode tag
    let (dev_name, mode_tag) = saved
        .split_once("::")
        .unwrap_or((saved, if is_input { "in:1ch" } else { "out:2ch" }));
    let channels = if mode_tag.ends_with("2ch") { 2 } else { 1 };

    for h in hosts {
        let iter = if is_input {
            h.input_devices()
        } else {
            h.output_devices()
        };
        if let Ok(mut it) = iter {
            if let Some(dev) = it.find(|d| stable_id(d).as_str() == dev_name) {
                return Some((dev, channels, saved.to_string()));
            }
        }
    }
    None
}

fn preferred_input_cfgs(channels: u16) -> [StreamConfig; 2] {
    // Try 10 ms @ 48k first; then same at stereo if initial request was mono-only (or vice versa).
    let (primary, secondary) = match channels {
        1 => (
            StreamConfig {
                channels: 1,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
            StreamConfig {
                channels: 2,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
        ),
        _ => (
            StreamConfig {
                channels: 2,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
            StreamConfig {
                channels: 1,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
        ),
    };
    [primary, secondary]
}

fn preferred_output_cfgs(channels: u16) -> [StreamConfig; 2] {
    // Prefer stereo; fallback to mono
    let (primary, secondary) = match channels {
        1 => (
            StreamConfig {
                channels: 1,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
            StreamConfig {
                channels: 2,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
        ),
        _ => (
            StreamConfig {
                channels: 2,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
            StreamConfig {
                channels: 1,
                sample_rate: SampleRate(48_000),
                buffer_size: BufferSize::Fixed(480),
            },
        ),
    };
    [primary, secondary]
}

fn first_supported_config(dev: &Device, is_input: bool) -> Option<StreamConfig> {
    let mut best: Option<StreamConfig> = None;
    let mut best_cost = i64::MAX;

    let iter = if is_input {
        dev.supported_input_configs().ok()?
    } else {
        dev.supported_output_configs().ok()?
    };
    for s in iter {
        let sr_min = s.min_sample_rate().0 as i64;
        let sr_max = s.max_sample_rate().0 as i64;

        // Choose 48k if in range; otherwise closest max
        let sr = if (sr_min..=sr_max).contains(&(48_000 as i64)) {
            48_000i64
        } else {
            sr_max
        };

        let ch = s.channels().clamp(1, 2); // expose ≤2ch for voice
        let cfg = StreamConfig {
            channels: ch,
            sample_rate: SampleRate(sr as u32),
            buffer_size: BufferSize::Default,
        };

        // Cost: prefer 48k, then 2ch for output / 1ch for input
        let sr_cost = (sr - 48_000).abs();
        let ch_cost = if is_input {
            (ch as i64 - 1).abs() * 1000
        } else {
            (2 - ch as i64).abs() * 1000
        };
        let fmt_cost = match s.sample_format() {
            SampleFormat::F32 => 0,
            _ => 100,
        };

        let cost = sr_cost + ch_cost + fmt_cost;
        if cost < best_cost {
            best_cost = cost;
            best = Some(cfg);
        }
    }
    best
}


fn first_supported_input_config(dev: &cpal::Device) -> Option<cpal::StreamConfig> {
    use cpal::{BufferSize, SampleFormat, SampleRate, SupportedStreamConfigRange};
    let mut best: Option<cpal::StreamConfig> = None;
    let mut best_cost = i64::MAX;

    let iter = dev.supported_input_configs().ok()?;
    for s in iter {
        let sr_min = s.min_sample_rate().0 as i64;
        let sr_max = s.max_sample_rate().0 as i64;
        let sr = if (sr_min..=sr_max).contains(&48_000i64) { 48_000i64 } else { sr_max };
        let ch = s.channels().clamp(1, 2);
        let cfg = cpal::StreamConfig { channels: ch, sample_rate: SampleRate(sr as u32), buffer_size: BufferSize::Default };
        let sr_cost = (sr - 48_000i64).abs();
        let ch_cost = (ch as i64 - 1i64).abs() * 1000;
        let fmt_cost = match s.sample_format() { SampleFormat::F32 => 0, _ => 100 };
        let cost = sr_cost + ch_cost + fmt_cost;
        if cost < best_cost {
            best_cost = cost;
            best = Some(cfg);
        }
    }
    best
}

fn first_supported_output_config(dev: &cpal::Device) -> Option<cpal::StreamConfig> {
    use cpal::{BufferSize, SampleFormat, SampleRate, SupportedStreamConfigRange};
    let mut best: Option<cpal::StreamConfig> = None;
    let mut best_cost = i64::MAX;

    let iter = dev.supported_output_configs().ok()?;
    for s in iter {
        let sr_min = s.min_sample_rate().0 as i64;
        let sr_max = s.max_sample_rate().0 as i64;
        let sr = if (sr_min..=sr_max).contains(&48_000i64) { 48_000i64 } else { sr_max };
        let ch = s.channels().clamp(1, 2);
        let cfg = cpal::StreamConfig { channels: ch, sample_rate: SampleRate(sr as u32), buffer_size: BufferSize::Default };
        let sr_cost = (sr - 48_000i64).abs();
        let ch_cost = (2i64 - ch as i64).abs() * 1000;
        let fmt_cost = match s.sample_format() { SampleFormat::F32 => 0, _ => 100 };
        let cost = sr_cost + ch_cost + fmt_cost;
        if cost < best_cost {
            best_cost = cost;
            best = Some(cfg);
        }
    }
    best
}

