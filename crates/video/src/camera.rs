//! Camera capture: a share can be a camera instead of a screen.
//!
//! Windows only, on Media Foundation — the system's own camera stack,
//! nothing to ship. [`list_cameras`] enumerates the video capture
//! devices; [`CameraCapturer`] opens one by its symbolic link (stable
//! across replugs, unlike a position in the list) and reads it on a
//! worker thread of its own. The source reader picks a decoder for
//! whatever the camera sends (YUY2, NV12, MJPEG…) and hands back RGB32,
//! which is the encoder's BGRA as it stands: the fourth byte is
//! undefined and nothing downstream reads it. The capturer exposes the
//! screen capturers' poll/frame API ([`crate::capture::Capturer`]), so
//! the share loop, the encoder and the viewers do not know the
//! difference; the encoder scales the picture into the quality preset
//! as it does a screen's.
//!
//! Of the formats the camera offers, the one nearest the preset is
//! chosen: smooth first (at least 24 fps, or the preset's rate if
//! lower), then the most pixels that fit inside the preset, then the
//! highest rate up to the preset's. Elsewhere there are no cameras yet:
//! [`list_cameras`] is empty and [`CameraCapturer::new`] fails.

#[cfg(target_os = "windows")]
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, PoisonError,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use crate::{capture::CaptureStatus, Frame};
use crate::{CaptureError, Quality};

/// A camera the system knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// What the system calls it ("Integrated Webcam").
    pub name: String,
    /// Its symbolic link: what [`CameraCapturer::new`] opens it by.
    pub id: String,
}

/// The cameras connected now, in the system's order.
///
/// # Errors
///
/// [`CaptureError::Capture`] if the system could not be asked.
#[cfg(target_os = "windows")]
pub fn list_cameras() -> Result<Vec<CameraInfo>, CaptureError> {
    // COM on a thread of our own: the caller's (the UI thread) may
    // already be in an apartment that is not the one wanted here.
    std::thread::Builder::new()
        .name("camera-list".into())
        .spawn(|| mf::with_mf(|| unsafe { mf::enumerate() }))
        .map_err(|e| CaptureError::Capture(format!("camera list: {e}")))?
        .join()
        .map_err(|_| CaptureError::Capture("camera list: the thread panicked".into()))?
}

/// No cameras off Windows (yet).
///
/// # Errors
///
/// Never.
#[cfg(not(target_os = "windows"))]
pub fn list_cameras() -> Result<Vec<CameraInfo>, CaptureError> {
    Ok(Vec::new())
}

/// How long dropping a capturer waits for its worker: one read of a
/// camera that stopped sending could block longer, and the share must
/// stop at once all the same (the worker then ends on its own).
#[cfg(target_os = "windows")]
const STOP_WAIT: Duration = Duration::from_secs(1);

/// What the worker and the capturer share.
#[cfg(target_os = "windows")]
#[derive(Default)]
struct Shared {
    /// The newest picture, not yet taken by [`CameraCapturer::poll`].
    latest: Mutex<Option<Frame>>,
    /// A picture's buffer to fill again (the one the capturer let go).
    spare: Mutex<Option<Vec<u8>>>,
    /// Why the camera stopped, once it has.
    failed: Mutex<Option<String>>,
    stop: AtomicBool,
}

#[cfg(target_os = "windows")]
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A camera, read on a worker thread; the share loop polls it like a
/// screen ([`crate::capture::Capturer::Camera`]).
#[cfg(target_os = "windows")]
pub struct CameraCapturer {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    /// The worker says here that it is finished.
    done_rx: mpsc::Receiver<()>,
    /// The picture held for the poll/frame API.
    last: Option<Frame>,
    start: Instant,
    /// What the system calls the camera, for the log.
    pub name: String,
}

#[cfg(target_os = "windows")]
impl CameraCapturer {
    /// Open the camera `id` ([`CameraInfo::id`]) in the format nearest
    /// `quality`, and start reading it.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Capture`] if the camera is gone, in use
    /// elsewhere, blocked by the system's privacy settings, or gives no
    /// picture the source reader can turn into RGB32.
    pub fn new(id: &str, quality: Quality) -> Result<Self, CaptureError> {
        let shared = Arc::new(Shared::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<String, String>>(1);
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker_shared = shared.clone();
        let id = id.to_string();
        let worker = std::thread::Builder::new()
            .name("camera-capture".into())
            .spawn(move || {
                mf::run(&id, quality, &worker_shared, &ready_tx);
                let _ = done_tx.send(());
            })
            .map_err(|e| CaptureError::Capture(format!("camera thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(name)) => Ok(Self {
                shared,
                worker: Some(worker),
                done_rx,
                last: None,
                start: Instant::now(),
                name,
            }),
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(CaptureError::Capture(e))
            }
            Err(_) => {
                let _ = worker.join();
                Err(CaptureError::Capture("the camera thread ended".into()))
            }
        }
    }

    /// See [`crate::capture::Capturer::poll`]: a picture the camera sent
    /// since the last poll is [`CaptureStatus::Fresh`].
    ///
    /// # Errors
    ///
    /// [`CaptureError::Capture`] once the camera stopped (unplugged,
    /// taken by the system).
    pub fn poll(&mut self) -> Result<CaptureStatus, CaptureError> {
        if let Some(e) = lock(&self.shared.failed).take() {
            return Err(CaptureError::Capture(e));
        }
        let fresh = lock(&self.shared.latest).take();
        match fresh {
            Some(mut frame) => {
                frame.timestamp_ms = self.start.elapsed().as_millis() as u64;
                // The picture let go is the worker's next buffer.
                if let Some(old) = self.last.replace(frame) {
                    *lock(&self.shared.spare) = Some(old.data);
                }
                Ok(CaptureStatus::Fresh)
            }
            None if self.last.is_some() => Ok(CaptureStatus::Unchanged),
            None => Ok(CaptureStatus::Empty),
        }
    }

    /// See [`crate::capture::Capturer::frame`].
    pub fn frame(&mut self, advance_timestamp: bool) -> Option<&Frame> {
        if advance_timestamp && let Some(f) = &mut self.last {
            f.timestamp_ms = self.start.elapsed().as_millis() as u64;
        }
        self.last.as_ref()
    }
}

#[cfg(target_os = "windows")]
impl Drop for CameraCapturer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            if self.done_rx.recv_timeout(STOP_WAIT).is_ok() {
                let _ = worker.join();
            } else {
                log::warn!("camera: the reader did not stop in time; leaving it to end on its own");
            }
        }
    }
}

/// No cameras off Windows (yet): there is nothing to open.
#[cfg(not(target_os = "windows"))]
pub struct CameraCapturer;

#[cfg(not(target_os = "windows"))]
impl CameraCapturer {
    /// Always fails: cameras are Windows-only for now.
    ///
    /// # Errors
    ///
    /// Always [`CaptureError::Capture`].
    pub fn new(_id: &str, _quality: Quality) -> Result<Self, CaptureError> {
        Err(CaptureError::Capture(
            "camera sharing is not available on this system".into(),
        ))
    }
}

/// The formats a camera offers, scored for `quality` (higher is
/// better): smooth, then the most pixels that fit, then the rate.
/// Pure arithmetic, tested below; `fps` may be fractional (29.97).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn format_score(width: u32, height: u32, fps: f32, quality: Quality) -> (bool, bool, i64, u32) {
    let fits = width <= quality.width() && height <= quality.height();
    let wanted = quality.fps() as f32;
    let capped = fps.min(wanted);
    let smooth = capped + 0.5 >= wanted.min(24.0);
    let area = i64::from(width) * i64::from(height);
    // Fitting: more is better. Too big: the smallest of those is.
    let size = if fits { area } else { -area };
    (fits, smooth, size, (capped * 100.0) as u32)
}

#[cfg(target_os = "windows")]
mod mf {
    use windows::core::{Interface, GUID, PWSTR};
    use windows::Win32::Media::MediaFoundation::{
        IMF2DBuffer, IMFActivate, IMFAttributes, IMFMediaSource, IMFMediaType, IMFSample,
        IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
        MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video, MFShutdown,
        MFStartup, MFVideoFormat_RGB32, MFSTARTUP_LITE, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_DEFAULT_STRIDE,
        MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
        MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
        MF_SOURCE_READERF_ERROR, MF_SOURCE_READER_ALL_STREAMS,
        MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
    };
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
    };

    use super::{format_score, lock, CameraInfo, Shared};
    use crate::{CaptureError, Frame, PixelFormat, Quality};

    // The constants are negative i32 sentinels in the API; the methods
    // take them as u32.
    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;

    /// COM and Media Foundation up for `f` on this thread, down after.
    pub(super) fn with_mf<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, CaptureError> {
        unsafe {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let result = match MFStartup(MF_VERSION, MFSTARTUP_LITE) {
                Ok(()) => {
                    let r = f();
                    let _ = MFShutdown();
                    r
                }
                Err(e) => Err(format!("Media Foundation: {e}")),
            };
            if com {
                CoUninitialize();
            }
            result.map_err(CaptureError::Capture)
        }
    }

    /// Every video capture device, as activation objects.
    unsafe fn devices() -> Result<Vec<IMFActivate>, String> {
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&raw mut attrs, 1).map_err(|e| format!("attributes: {e}"))?;
            let attrs = attrs.ok_or("attributes: none")?;
            attrs
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .map_err(|e| format!("attributes: {e}"))?;
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFEnumDeviceSources(&attrs, &raw mut list, &raw mut count)
                .map_err(|e| format!("camera list: {e}"))?;
            let mut out = Vec::with_capacity(count as usize);
            if !list.is_null() {
                for i in 0..count as usize {
                    // Each entry's reference moves into `out`; the array
                    // itself is the system's, freed below.
                    if let Some(activate) = list.add(i).read() {
                        out.push(activate);
                    }
                }
                CoTaskMemFree(Some(list.cast_const().cast()));
            }
            Ok(out)
        }
    }

    /// A string attribute, if it is there.
    unsafe fn string(attrs: &IMFAttributes, key: &GUID) -> Option<String> {
        unsafe {
            let mut value = PWSTR::null();
            let mut len = 0u32;
            attrs
                .GetAllocatedString(key, &raw mut value, &raw mut len)
                .ok()?;
            let s = value.to_string().ok();
            CoTaskMemFree(Some(value.0.cast_const().cast()));
            s
        }
    }

    pub(super) unsafe fn enumerate() -> Result<Vec<CameraInfo>, String> {
        unsafe {
            Ok(devices()?
                .iter()
                .enumerate()
                .map(|(i, a)| CameraInfo {
                    name: string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                        .unwrap_or_else(|| format!("Camera {}", i + 1)),
                    id: string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)
                        .unwrap_or_default(),
                })
                .collect())
        }
    }

    /// A packed `(hi, lo)` pair attribute (frame size, frame rate).
    unsafe fn pair(attrs: &IMFAttributes, key: &GUID) -> Option<(u32, u32)> {
        let v = unsafe { attrs.GetUINT64(key) }.ok()?;
        Some(((v >> 32) as u32, v as u32))
    }

    /// The native format nearest `quality` ([`format_score`]).
    unsafe fn best_native(reader: &IMFSourceReader, quality: Quality) -> Option<IMFMediaType> {
        let mut best: Option<((bool, bool, i64, u32), IMFMediaType)> = None;
        for i in 0.. {
            let Ok(t) = (unsafe { reader.GetNativeMediaType(VIDEO, i) }) else {
                break;
            };
            let Some((w, h)) = (unsafe { pair(&t, &MF_MT_FRAME_SIZE) }) else {
                continue;
            };
            let fps = unsafe { pair(&t, &MF_MT_FRAME_RATE) }
                .filter(|&(_, den)| den > 0)
                .map_or(30.0, |(num, den)| num as f32 / den as f32);
            let score = format_score(w, h, fps, quality);
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, t));
            }
        }
        best.map(|(_, t)| t)
    }

    /// An open camera.
    struct Opened {
        reader: IMFSourceReader,
        source: IMFMediaSource,
        name: String,
        width: u32,
        height: u32,
        /// Bytes from one row to the next in a plain buffer; negative
        /// when the picture is stored bottom-up.
        stride: i32,
    }

    unsafe fn open(id: &str, quality: Quality) -> Result<Opened, String> {
        unsafe {
            let devices = devices()?;
            let activate = devices
                .iter()
                .find(|a| {
                    string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).as_deref()
                        == Some(id)
                })
                .ok_or("the camera is not connected")?;
            let name = string(activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                .unwrap_or_else(|| "camera".into());
            let source: IMFMediaSource = activate.ActivateObject().map_err(|e| {
                format!(
                    "{name} would not open ({e}) — in use by another app, or blocked in \
                     Windows' camera privacy settings?"
                )
            })?;
            let opened = (|| {
                let mut attrs: Option<IMFAttributes> = None;
                MFCreateAttributes(&raw mut attrs, 1).map_err(|e| format!("attributes: {e}"))?;
                let attrs = attrs.ok_or("attributes: none")?;
                // The reader inserts the decoder and the colour converter.
                attrs
                    .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                    .map_err(|e| format!("attributes: {e}"))?;
                let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)
                    .map_err(|e| format!("{name}: no reader ({e})"))?;
                let _ = reader.SetStreamSelection(ALL, false);
                reader
                    .SetStreamSelection(VIDEO, true)
                    .map_err(|e| format!("{name}: no video stream ({e})"))?;
                if let Some(native) = best_native(&reader, quality)
                    && let Err(e) = reader.SetCurrentMediaType(VIDEO, None, &native)
                {
                    log::debug!("camera: keeping the default format ({e})");
                }
                let current = reader
                    .GetCurrentMediaType(VIDEO)
                    .map_err(|e| format!("{name}: no format ({e})"))?;
                let size = current
                    .GetUINT64(&MF_MT_FRAME_SIZE)
                    .map_err(|e| format!("{name}: no frame size ({e})"))?;
                let want: IMFMediaType =
                    MFCreateMediaType().map_err(|e| format!("media type: {e}"))?;
                want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .map_err(|e| format!("media type: {e}"))?;
                want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                    .map_err(|e| format!("media type: {e}"))?;
                want.SetUINT64(&MF_MT_FRAME_SIZE, size)
                    .map_err(|e| format!("media type: {e}"))?;
                reader
                    .SetCurrentMediaType(VIDEO, None, &want)
                    .map_err(|e| format!("{name} gives no picture the app can convert ({e})"))?;
                let (width, height, stride) = layout(&reader)?;
                Ok::<_, String>((reader, width, height, stride))
            })();
            match opened {
                Ok((reader, width, height, stride)) => Ok(Opened {
                    reader,
                    source,
                    name,
                    width,
                    height,
                    stride,
                }),
                Err(e) => {
                    let _ = source.Shutdown();
                    Err(e)
                }
            }
        }
    }

    /// The RGB32 output's size and row stride, as the reader has it now.
    unsafe fn layout(reader: &IMFSourceReader) -> Result<(u32, u32, i32), String> {
        unsafe {
            let actual = reader
                .GetCurrentMediaType(VIDEO)
                .map_err(|e| format!("output format: {e}"))?;
            let (width, height) =
                pair(&actual, &MF_MT_FRAME_SIZE).ok_or("output format: no frame size")?;
            if width == 0 || height == 0 {
                return Err("output format: an empty picture".into());
            }
            // Uncompressed RGB is bottom-up unless the type says.
            let stride = actual
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map_or_else(|_| -(width as i32 * 4), |s| s as i32);
            Ok((width, height, stride))
        }
    }

    /// The worker: open, say so on `ready`, read until told to stop.
    pub(super) fn run(
        id: &str,
        quality: Quality,
        shared: &Shared,
        ready: &std::sync::mpsc::SyncSender<Result<String, String>>,
    ) {
        let outcome = with_mf(|| {
            let cam = match unsafe { open(id, quality) } {
                Ok(cam) => cam,
                Err(e) => {
                    let _ = ready.send(Err(e));
                    return Ok(());
                }
            };
            log::info!(
                "camera: '{}' at {}x{} (RGB32 via the source reader), target={quality}",
                cam.name,
                cam.width,
                cam.height
            );
            let _ = ready.send(Ok(cam.name.clone()));
            let result = unsafe { read(cam, shared) };
            if let Err(e) = result {
                log::warn!("camera: {e}");
                *lock(&shared.failed) = Some(e);
            }
            Ok(())
        });
        if let Err(e) = outcome {
            // Before the camera opened: the opener is waiting for this.
            let _ = ready.send(Err(e.to_string()));
        }
    }

    unsafe fn read(mut cam: Opened, shared: &Shared) -> Result<(), String> {
        let result = (|| {
            while !shared.stop.load(std::sync::atomic::Ordering::Relaxed) {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                unsafe {
                    cam.reader.ReadSample(
                        VIDEO,
                        0,
                        None,
                        Some(&raw mut flags),
                        None,
                        Some(&raw mut sample),
                    )
                }
                .map_err(|e| format!("{} stopped: {e}", cam.name))?;
                let flags = flags as i32;
                if flags & MF_SOURCE_READERF_ERROR.0 != 0 {
                    return Err(format!("{} reported an error", cam.name));
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                    return Err(format!("{} ended (unplugged?)", cam.name));
                }
                if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 != 0 {
                    (cam.width, cam.height, cam.stride) = unsafe { layout(&cam.reader) }?;
                }
                let Some(sample) = sample else {
                    continue;
                };
                let data = shared_buffer(shared, cam.width, cam.height);
                let data = unsafe { copy_picture(&sample, &cam, data) }?;
                *lock(&shared.latest) = Some(Frame {
                    data,
                    width: cam.width,
                    height: cam.height,
                    timestamp_ms: 0,
                    format: PixelFormat::Bgra,
                });
            }
            Ok(())
        })();
        // The light goes off.
        let _ = unsafe { cam.source.Shutdown() };
        result
    }

    /// A buffer for the next picture: the spare one back from the
    /// capturer, or a new one.
    fn shared_buffer(shared: &Shared, width: u32, height: u32) -> Vec<u8> {
        let mut data = lock(&shared.spare).take().unwrap_or_default();
        data.resize(width as usize * height as usize * 4, 0);
        data
    }

    /// The sample's picture into `data`, top row first, rows packed.
    unsafe fn copy_picture(
        sample: &IMFSample,
        cam: &Opened,
        mut data: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        unsafe {
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| format!("picture: {e}"))?;
            let row = cam.width as usize * 4;
            let rows = cam.height as usize;
            // A 2-D buffer knows its own pitch (and which way up it is).
            if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
                let mut first: *mut u8 = std::ptr::null_mut();
                let mut pitch = 0i32;
                b2.Lock2D(&raw mut first, &raw mut pitch)
                    .map_err(|e| format!("picture: {e}"))?;
                if !first.is_null() && pitch.unsigned_abs() as usize >= row {
                    for (y, dst) in data.chunks_exact_mut(row).take(rows).enumerate() {
                        let src = first.offset(y as isize * pitch as isize);
                        std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), row);
                    }
                }
                let _ = b2.Unlock2D();
                return Ok(data);
            }
            let mut base: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&raw mut base, None, Some(&raw mut len))
                .map_err(|e| format!("picture: {e}"))?;
            let pitch = cam.stride.unsigned_abs() as usize;
            if !base.is_null() && pitch >= row && len as usize >= pitch * rows {
                for (y, dst) in data.chunks_exact_mut(row).take(rows).enumerate() {
                    // Bottom-up: the first row in memory is the last.
                    let line = if cam.stride < 0 { rows - 1 - y } else { y };
                    std::ptr::copy_nonoverlapping(base.add(line * pitch), dst.as_mut_ptr(), row);
                }
            }
            let _ = buffer.Unlock();
            Ok(data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smooth_format_beats_a_bigger_slow_one() {
        // A USB 2 webcam: 1080p raw at 5 fps, or 720p at 30.
        let slow = format_score(1920, 1080, 5.0, Quality::FHD30);
        let smooth = format_score(1280, 720, 30.0, Quality::FHD30);
        assert!(smooth > slow);
        // 1080p MJPEG at 30 beats both.
        assert!(format_score(1920, 1080, 30.0, Quality::FHD30) > smooth);
    }

    #[test]
    fn a_format_inside_the_preset_beats_one_past_it() {
        let inside = format_score(1280, 720, 30.0, Quality::HD30);
        let past = format_score(1920, 1080, 30.0, Quality::HD30);
        assert!(inside > past);
        // Past it, the smaller of two is nearer.
        assert!(
            format_score(1920, 1080, 30.0, Quality::HD30)
                > format_score(3840, 2160, 30.0, Quality::HD30)
        );
    }

    #[test]
    fn the_rate_counts_up_to_the_presets_only() {
        let at_preset = format_score(1280, 720, 30.0, Quality::HD30);
        let faster = format_score(1280, 720, 60.0, Quality::HD30);
        assert_eq!(at_preset, faster);
        assert!(
            format_score(1280, 720, 60.0, Quality::HD60)
                > format_score(1280, 720, 30.0, Quality::HD60)
        );
        // 29.97 is as smooth as 30.
        assert!(format_score(1280, 720, 29.97, Quality::HD30).1);
    }

    /// The machine's real cameras (enumeration only: nothing is opened).
    /// `cargo test -p video lists_this_machines_cameras -- --ignored --nocapture`
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "needs the machine's devices"]
    fn lists_this_machines_cameras() {
        let cameras = list_cameras().expect("the camera list");
        for c in &cameras {
            println!("camera: {} [{}]", c.name, c.id);
            assert!(!c.id.is_empty(), "{} has no id to open it by", c.name);
        }
        println!("{} camera(s)", cameras.len());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn no_cameras_off_windows() {
        assert!(list_cameras().unwrap().is_empty());
        assert!(CameraCapturer::new("x", Quality::HD30).is_err());
    }
}
