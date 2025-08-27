// voice/diagnostics.rs - Enhanced pipeline diagnostics with resampling metrics
use log::{error, info, warn};
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ============================================================================
// Core Diagnostic Types
// ============================================================================

#[derive(Debug, Clone)]
pub struct StageMetrics {
    pub name: &'static str,
    pub samples_in: usize,
    pub samples_out: usize,
    pub timestamp: Instant,
    pub buffer_occupancy: usize,
    pub processing_time_us: u64,
}

#[derive(Debug, Clone)]
pub struct Anomaly {
    pub stage: &'static str,
    pub issue: AnomalyType,
    pub value: f64,
    pub expected_range: (f64, f64),
    pub timestamp: Instant,
}

#[derive(Debug, Clone)]
pub enum AnomalyType {
    LatencySpike,
    BufferOverflow,
    BufferUnderflow,
    SampleRateMismatch,
    SilentAudio,
    ClippedAudio,
    FrameSizeMismatch,
    ThreadStarvation,
    RingBufferDrift,
    ResamplerUnderrun,
    ResamplerOverrun,
    DiscontinuityDetected,
    DriftExcessive,
}

impl std::fmt::Display for AnomalyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnomalyType::LatencySpike => write!(f, "Latency Spike"),
            AnomalyType::BufferOverflow => write!(f, "Buffer Overflow"),
            AnomalyType::BufferUnderflow => write!(f, "Buffer Underflow"),
            AnomalyType::SampleRateMismatch => write!(f, "Sample Rate Mismatch"),
            AnomalyType::SilentAudio => write!(f, "Silent Audio"),
            AnomalyType::ClippedAudio => write!(f, "Clipped Audio"),
            AnomalyType::FrameSizeMismatch => write!(f, "Frame Size Mismatch"),
            AnomalyType::ThreadStarvation => write!(f, "Thread Starvation"),
            AnomalyType::RingBufferDrift => write!(f, "Ring Buffer Drift"),
            AnomalyType::ResamplerUnderrun => write!(f, "Resampler Underrun"),
            AnomalyType::ResamplerOverrun => write!(f, "Resampler Overrun"),
            AnomalyType::DiscontinuityDetected => write!(f, "Audio Discontinuity"),
            AnomalyType::DriftExcessive => write!(f, "Excessive Drift"),
        }
    }
}

// ============================================================================
// Audio Fingerprinting with Discontinuity Detection
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct AudioFingerprint {
    pub dc_offset: f32,
    pub avg_magnitude: f32,
    pub rms: f32,
    pub silent_samples: u32,
    pub peak: f32,
    pub zero_crossings: u32,
    pub last_sample: f32,
    pub discontinuities: u32,
}

impl AudioFingerprint {
    pub fn from_samples(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }

        let len = samples.len() as f32;
        let dc_offset = samples.iter().sum::<f32>() / len;
        let avg_magnitude = samples.iter().map(|x| x.abs()).sum::<f32>() / len;
        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / len).sqrt();
        let silent_samples = samples.iter().filter(|&&x| x.abs() < 0.001).count() as u32;
        let peak = samples.iter().map(|x| x.abs()).fold(0.0f32, f32::max);

        let mut zero_crossings = 0u32;
        let mut discontinuities = 0u32;

        for i in 1..samples.len() {
            // Zero crossing detection
            if samples[i - 1].signum() != samples[i].signum() && samples[i].abs() > 0.001 {
                zero_crossings += 1;
            }

            // Discontinuity detection - sudden jumps
            let diff = (samples[i] - samples[i - 1]).abs();
            if diff > 0.5 && samples[i].abs() > 0.01 && samples[i - 1].abs() > 0.01 {
                discontinuities += 1;
            }
        }

        Self {
            dc_offset,
            avg_magnitude,
            rms,
            silent_samples,
            peak,
            zero_crossings,
            last_sample: samples.last().copied().unwrap_or(0.0),
            discontinuities,
        }
    }

    pub fn check_continuity(&self, prev: &Self, sample_rate: u32) -> Option<Anomaly> {
        // Check for discontinuity between buffers
        let expected_diff = 1.0 / sample_rate as f32; // Max expected sample-to-sample difference
        let actual_diff = (self.last_sample - prev.last_sample).abs();

        if actual_diff > 0.5 && self.rms > 0.01 && prev.rms > 0.01 {
            Some(Anomaly {
                stage: "continuity",
                issue: AnomalyType::DiscontinuityDetected,
                value: actual_diff as f64,
                expected_range: (0.0, 0.5),
                timestamp: Instant::now(),
            })
        } else {
            None
        }
    }

    pub fn is_silent(&self) -> bool {
        self.rms < 0.001
    }

    pub fn is_clipping(&self) -> bool {
        self.peak > 0.98
    }

    pub fn has_discontinuities(&self) -> bool {
        self.discontinuities > 0
    }
}

impl Default for AudioFingerprint {
    fn default() -> Self {
        Self {
            dc_offset: 0.0,
            avg_magnitude: 0.0,
            rms: 0.0,
            silent_samples: 0,
            peak: 0.0,
            zero_crossings: 0,
            last_sample: 0.0,
            discontinuities: 0,
        }
    }
}

// ============================================================================
// Resampler Performance Tracking
// ============================================================================

#[derive(Debug, Clone)]
pub struct ResamplerMetrics {
    pub stage: &'static str,
    pub input_frames: usize,
    pub output_frames: usize,
    pub expected_output: usize,
    pub underruns: u64,
    pub overruns: u64,
    pub ratio: f64,
    pub actual_ratio: f64,
    pub drift_ppm: f64,
    pub processing_time_us: u64,

    // Add cumulative tracking for proper drift calculation
    pub total_input: u64,
    pub total_output: u64,
}

impl ResamplerMetrics {
    pub fn new(stage: &'static str) -> Self {
        Self {
            stage,
            input_frames: 0,
            output_frames: 0,
            expected_output: 0,
            underruns: 0,
            overruns: 0,
            ratio: 1.0,
            actual_ratio: 1.0,
            drift_ppm: 0.0,
            processing_time_us: 0,
            total_input: 0,
            total_output: 0,
        }
    }

    pub fn set_expected_ratio(&mut self, ratio: f64) {
        self.ratio = ratio;
    }

    pub fn update(&mut self, input: usize, output: usize, expected: usize, time_us: u64) {
        self.input_frames = input;
        self.output_frames = output;
        self.expected_output = expected;
        self.processing_time_us = time_us;

        // Update totals
        self.total_input += input as u64;
        self.total_output += output as u64;

        // Calculate actual ratio from this batch
        if input > 0 {
            self.actual_ratio = output as f64 / input as f64;
        }

        if output < expected {
            self.underruns += 1;
        } else if output > expected {
            self.overruns += 1;
        }
    }

    pub fn calculate_drift(&mut self) -> f64 {
        // Only calculate drift if we have meaningful data
        if self.total_input < 1000 || self.ratio == 0.0 {
            self.drift_ppm = 0.0;
            return 0.0;
        }

        // Calculate long-term average ratio
        let long_term_ratio = self.total_output as f64 / self.total_input as f64;

        // Compare to expected ratio
        if self.ratio > 0.0 {
            // Calculate PPM difference from expected
            self.drift_ppm = ((long_term_ratio / self.ratio) - 1.0) * 1_000_000.0;

            // Clamp to reasonable range
            self.drift_ppm = self.drift_ppm.clamp(-10000.0, 10000.0);
        }

        self.drift_ppm
    }
}

// ============================================================================
// Latency Waterfall Tracking
// ============================================================================

#[derive(Debug, Clone)]
pub struct LatencyWaterfall {
    pub input_callback: Duration,
    pub input_ring: Duration,
    pub input_resampler: Duration,
    pub vad: Duration,
    pub encode: Duration,
    pub network: Duration,
    pub decode: Duration,
    pub jitter_buffer: Duration,
    pub output_resampler: Duration,
    pub output_ring: Duration,
    pub output_callback: Duration,
}

impl LatencyWaterfall {
    pub fn new() -> Self {
        Self {
            input_callback: Duration::ZERO,
            input_ring: Duration::ZERO,
            input_resampler: Duration::ZERO,
            vad: Duration::ZERO,
            encode: Duration::ZERO,
            network: Duration::ZERO,
            decode: Duration::ZERO,
            jitter_buffer: Duration::ZERO,
            output_resampler: Duration::ZERO,
            output_ring: Duration::ZERO,
            output_callback: Duration::ZERO,
        }
    }

    pub fn total(&self) -> Duration {
        self.input_callback
            + self.input_ring
            + self.input_resampler
            + self.vad
            + self.encode
            + self.network
            + self.decode
            + self.jitter_buffer
            + self.output_resampler
            + self.output_ring
            + self.output_callback
    }

    pub fn report(&self) -> String {
        format!(
            "📊 Latency Waterfall (Total: {:.1}ms):\n  \
             Input Callback:   {:>6.2}ms\n  \
             Input Ring:       {:>6.2}ms\n  \
             Input Resampler:  {:>6.2}ms\n  \
             VAD:              {:>6.2}ms\n  \
             Encode:           {:>6.2}ms\n  \
             Network:          {:>6.2}ms\n  \
             Decode:           {:>6.2}ms\n  \
             Jitter Buffer:    {:>6.2}ms\n  \
             Output Resampler: {:>6.2}ms\n  \
             Output Ring:      {:>6.2}ms\n  \
             Output Callback:  {:>6.2}ms",
            self.total().as_secs_f32() * 1000.0,
            self.input_callback.as_secs_f32() * 1000.0,
            self.input_ring.as_secs_f32() * 1000.0,
            self.input_resampler.as_secs_f32() * 1000.0,
            self.vad.as_secs_f32() * 1000.0,
            self.encode.as_secs_f32() * 1000.0,
            self.network.as_secs_f32() * 1000.0,
            self.decode.as_secs_f32() * 1000.0,
            self.jitter_buffer.as_secs_f32() * 1000.0,
            self.output_resampler.as_secs_f32() * 1000.0,
            self.output_ring.as_secs_f32() * 1000.0,
            self.output_callback.as_secs_f32() * 1000.0,
        )
    }
}

// ============================================================================
// Ring Buffer Monitor
// ============================================================================

pub struct RingBufferMonitor {
    name: &'static str,
    pub samples_pushed: AtomicU64,
    pub samples_popped: AtomicU64,
    last_check: Mutex<Instant>,
    last_pushed: AtomicU64,
    last_popped: AtomicU64,

    // New metrics
    peak_occupancy: AtomicUsize,
    min_occupancy: AtomicUsize,
    underrun_events: AtomicU64,
    overrun_events: AtomicU64,
}

impl RingBufferMonitor {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            samples_pushed: AtomicU64::new(0),
            samples_popped: AtomicU64::new(0),
            last_check: Mutex::new(Instant::now()),
            last_pushed: AtomicU64::new(0),
            last_popped: AtomicU64::new(0),
            peak_occupancy: AtomicUsize::new(0),
            min_occupancy: AtomicUsize::new(usize::MAX),
            underrun_events: AtomicU64::new(0),
            overrun_events: AtomicU64::new(0),
        }
    }

    pub fn record_push(&self, count: usize) {
        self.samples_pushed
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub fn record_pop(&self, count: usize) {
        self.samples_popped
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub fn record_underrun(&self) {
        self.underrun_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_overrun(&self) {
        self.overrun_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn update_occupancy(&self, current: usize) {
        // Update peak
        let mut peak = self.peak_occupancy.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_occupancy.compare_exchange_weak(
                peak,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => peak = x,
            }
        }

        // Update min
        let mut min = self.min_occupancy.load(Ordering::Relaxed);
        while current < min {
            match self.min_occupancy.compare_exchange_weak(
                min,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => min = x,
            }
        }
    }

    pub fn check_rate_mismatch(&self) -> Option<Anomaly> {
        let mut last_check = self.last_check.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(*last_check);

        if elapsed < Duration::from_secs(1) {
            return None;
        }

        let pushed = self.samples_pushed.load(Ordering::Relaxed);
        let popped = self.samples_popped.load(Ordering::Relaxed);
        let last_pushed = self.last_pushed.load(Ordering::Relaxed);
        let last_popped = self.last_popped.load(Ordering::Relaxed);

        let push_rate = (pushed - last_pushed) as f64 / elapsed.as_secs_f64();
        let pop_rate = (popped - last_popped) as f64 / elapsed.as_secs_f64();

        self.last_pushed.store(pushed, Ordering::Relaxed);
        self.last_popped.store(popped, Ordering::Relaxed);
        *last_check = now;

        let rate_diff = (push_rate - pop_rate).abs();
        if rate_diff > 480.0 {
            // More than 10ms worth of drift at 48kHz
            Some(Anomaly {
                stage: self.name,
                issue: AnomalyType::RingBufferDrift,
                value: rate_diff,
                expected_range: (0.0, 480.0),
                timestamp: now,
            })
        } else {
            None
        }
    }

    pub fn get_occupancy(&self) -> i64 {
        let pushed = self.samples_pushed.load(Ordering::Relaxed) as i64;
        let popped = self.samples_popped.load(Ordering::Relaxed) as i64;
        pushed - popped
    }

    pub fn get_stats(&self) -> String {
        let occupancy = self.get_occupancy();
        let underruns = self.underrun_events.load(Ordering::Relaxed);
        let overruns = self.overrun_events.load(Ordering::Relaxed);
        let peak = self.peak_occupancy.load(Ordering::Relaxed);
        let min = self.min_occupancy.load(Ordering::Relaxed);

        format!(
            "{}: occupancy={} (min={}, peak={}), underruns={}, overruns={}",
            self.name,
            occupancy,
            if min == usize::MAX { 0 } else { min },
            peak,
            underruns,
            overruns
        )
    }
}

// ============================================================================
// Test Signal Generator
// ============================================================================

pub enum TestSignal {
    Sine440Hz,
    Sine1kHz,
    Chirp,
    Impulse,
    WhiteNoise,
    CountingPattern,
}

impl TestSignal {
    pub fn generate(&self, samples: usize, sample_rate: u32) -> Vec<f32> {
        match self {
            TestSignal::Sine440Hz => (0..samples)
                .map(|i| (i as f32 * 440.0 * 2.0 * PI / sample_rate as f32).sin() * 0.5)
                .collect(),
            TestSignal::Sine1kHz => (0..samples)
                .map(|i| (i as f32 * 1000.0 * 2.0 * PI / sample_rate as f32).sin() * 0.5)
                .collect(),
            TestSignal::Chirp => {
                (0..samples)
                    .map(|i| {
                        let t = i as f32 / sample_rate as f32;
                        let freq = 200.0 + t * 2000.0; // 200Hz to 2200Hz sweep
                        (t * freq * 2.0 * PI).sin() * 0.5
                    })
                    .collect()
            }
            TestSignal::Impulse => (0..samples)
                .map(|i| {
                    if i % (sample_rate as usize / 10) == 0 {
                        0.5
                    } else {
                        0.0
                    }
                })
                .collect(),
            TestSignal::WhiteNoise => {
                use rand::Rng;
                let mut rng = rand::thread_rng();
                (0..samples).map(|_| rng.gen_range(-0.5..0.5)).collect()
            }
            TestSignal::CountingPattern => (0..samples)
                .map(|i| ((i % 100) as f32 / 100.0 - 0.5))
                .collect(),
        }
    }
}

// ============================================================================
// Pipeline Diagnostics Manager
// ============================================================================

pub struct PipelineDiagnostics {
    enabled: Arc<AtomicBool>,
    stages: Arc<Mutex<VecDeque<StageMetrics>>>,
    pub anomalies: Arc<Mutex<VecDeque<Anomaly>>>,
    waterfall: Arc<Mutex<LatencyWaterfall>>,

    // Ring buffer monitors
    pub input_ring: Arc<RingBufferMonitor>,
    pub output_ring: Arc<RingBufferMonitor>,

    // Resampler metrics
    pub input_resampler: Arc<Mutex<ResamplerMetrics>>,
    pub output_resampler: Arc<Mutex<ResamplerMetrics>>,

    // Audio fingerprints for continuity checking
    pub last_input_fingerprint: Arc<Mutex<AudioFingerprint>>,
    pub last_output_fingerprint: Arc<Mutex<AudioFingerprint>>,

    // Thread timing
    pub last_input_callback: Arc<Mutex<Instant>>,
    pub last_output_callback: Arc<Mutex<Instant>>,

    // Test mode
    pub test_mode: Arc<Mutex<TestMode>>,

    // Callback counters
    pub input_callback_count: AtomicU64,
    pub output_callback_count: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TestMode {
    Disabled,
    Passthrough,
    Sine1kHz,
    Chirp,
    WhiteNoise,
}

impl PipelineDiagnostics {
    pub fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)), // Enable by default for debugging
            stages: Arc::new(Mutex::new(VecDeque::with_capacity(1000))),
            anomalies: Arc::new(Mutex::new(VecDeque::with_capacity(100))),
            waterfall: Arc::new(Mutex::new(LatencyWaterfall::new())),
            input_ring: Arc::new(RingBufferMonitor::new("input_ring")),
            output_ring: Arc::new(RingBufferMonitor::new("output_ring")),
            input_resampler: Arc::new(Mutex::new(ResamplerMetrics::new("input_resampler"))),
            output_resampler: Arc::new(Mutex::new(ResamplerMetrics::new("output_resampler"))),
            last_input_fingerprint: Arc::new(Mutex::new(AudioFingerprint::default())),
            last_output_fingerprint: Arc::new(Mutex::new(AudioFingerprint::default())),
            last_input_callback: Arc::new(Mutex::new(Instant::now())),
            last_output_callback: Arc::new(Mutex::new(Instant::now())),
            test_mode: Arc::new(Mutex::new(TestMode::Disabled)),
            input_callback_count: AtomicU64::new(0),
            output_callback_count: AtomicU64::new(0),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
        info!("🔍 Pipeline diagnostics enabled");
    }

    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Relaxed);
        info!("🔍 Pipeline diagnostics disabled");
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_test_mode(&self, mode: TestMode) {
        *self.test_mode.lock().unwrap() = mode;
        info!("🧪 Test mode set to: {:?}", mode);
    }

    pub fn get_test_mode(&self) -> TestMode {
        *self.test_mode.lock().unwrap()
    }

    pub fn record_stage(&self, metrics: StageMetrics) {
        if !self.is_enabled() {
            return;
        }

        let mut stages = self.stages.lock().unwrap();
        if stages.len() >= 1000 {
            stages.pop_front();
        }
        stages.push_back(metrics);
    }

    pub fn record_anomaly(&self, anomaly: Anomaly) {
        let mut anomalies = self.anomalies.lock().unwrap();
        if anomalies.len() >= 100 {
            anomalies.pop_front();
        }

        warn!(
            "🚨 {} - {}: value={:.2} (expected {:.2}-{:.2})",
            anomaly.stage,
            anomaly.issue,
            anomaly.value,
            anomaly.expected_range.0,
            anomaly.expected_range.1
        );

        anomalies.push_back(anomaly);
    }

    pub fn check_callback_timing(&self, stage: &'static str, is_input: bool) -> Option<Anomaly> {
        let last_time = if is_input {
            self.last_input_callback.clone()
        } else {
            self.last_output_callback.clone()
        };

        let mut last = last_time.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(*last);
        *last = now;

        // Increment callback counter
        if is_input {
            self.input_callback_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.output_callback_count.fetch_add(1, Ordering::Relaxed);
        }

        // Only report if significantly late (>20ms)
        if elapsed > Duration::from_millis(20) {
            Some(Anomaly {
                stage,
                issue: AnomalyType::ThreadStarvation,
                value: elapsed.as_millis() as f64,
                expected_range: (0.0, 20.0),
                timestamp: now,
            })
        } else {
            None
        }
    }

    pub fn check_audio_continuity(
        &self,
        stage: &'static str,
        samples: &[f32],
        is_input: bool,
    ) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        let fp = AudioFingerprint::from_samples(samples);

        // Check for discontinuities within the buffer
        if fp.has_discontinuities() {
            anomalies.push(Anomaly {
                stage,
                issue: AnomalyType::DiscontinuityDetected,
                value: fp.discontinuities as f64,
                expected_range: (0.0, 0.0),
                timestamp: Instant::now(),
            });
        }

        // Check for clipping
        if fp.is_clipping() {
            anomalies.push(Anomaly {
                stage,
                issue: AnomalyType::ClippedAudio,
                value: fp.peak as f64,
                expected_range: (0.0, 0.98),
                timestamp: Instant::now(),
            });
        }

        // Check continuity with previous buffer
        let last_fp = if is_input {
            self.last_input_fingerprint.clone()
        } else {
            self.last_output_fingerprint.clone()
        };

        let mut last = last_fp.lock().unwrap();
        if let Some(anomaly) = fp.check_continuity(&*last, 48000) {
            anomalies.push(anomaly);
        }
        *last = fp;

        anomalies
    }

    pub fn update_waterfall(&self, update_fn: impl FnOnce(&mut LatencyWaterfall)) {
        if !self.is_enabled() {
            return;
        }

        let mut waterfall = self.waterfall.lock().unwrap();
        update_fn(&mut waterfall);
    }

    pub fn update_resampler_metrics(
        &self,
        is_input: bool,
        input: usize,
        output: usize,
        expected: usize,
        time_us: u64,
        expected_ratio: f64,
    ) {
        if !self.is_enabled() {
            return;
        }

        let metrics = if is_input {
            self.input_resampler.clone()
        } else {
            self.output_resampler.clone()
        };

        let mut m = metrics.lock().unwrap();

        // Set the expected ratio if not already set
        if m.ratio == 1.0 && expected_ratio != 1.0 {
            m.set_expected_ratio(expected_ratio);
        }

        m.update(input, output, expected, time_us);

        let drift = m.calculate_drift();

        // Only report excessive drift if it's actually excessive and we have enough samples
        if drift.abs() > 100.0 && m.total_input > 10000 {
            self.record_anomaly(Anomaly {
                stage: m.stage,
                issue: AnomalyType::DriftExcessive,
                value: drift,
                expected_range: (-100.0, 100.0),
                timestamp: Instant::now(),
            });
        }
    }

    pub fn get_report(&self) -> String {
        let waterfall = self.waterfall.lock().unwrap();
        let anomalies = self.anomalies.lock().unwrap();
        let input_resampler = self.input_resampler.lock().unwrap();
        let output_resampler = self.output_resampler.lock().unwrap();

        let mut report = String::new();

        // Waterfall
        report.push_str(&waterfall.report());

        // Ring buffers
        report.push_str("\n\n📊 Ring Buffers:\n");
        report.push_str(&format!("  Input:  {}\n", self.input_ring.get_stats()));
        report.push_str(&format!("  Output: {}\n", self.output_ring.get_stats()));

        // Resamplers
        report.push_str("\n🔄 Resamplers:\n");
        report.push_str(&format!(
            "  Input:  {} → {} (expected {}), drift={:.1}ppm, underruns={}, time={:.2}ms\n",
            input_resampler.input_frames,
            input_resampler.output_frames,
            input_resampler.expected_output,
            input_resampler.drift_ppm,
            input_resampler.underruns,
            input_resampler.processing_time_us as f32 / 1000.0
        ));
        report.push_str(&format!(
            "  Output: {} → {} (expected {}), drift={:.1}ppm, underruns={}, time={:.2}ms\n",
            output_resampler.input_frames,
            output_resampler.output_frames,
            output_resampler.expected_output,
            output_resampler.drift_ppm,
            output_resampler.underruns,
            output_resampler.processing_time_us as f32 / 1000.0
        ));

        // Callback rates
        let input_callbacks = self.input_callback_count.load(Ordering::Relaxed);
        let output_callbacks = self.output_callback_count.load(Ordering::Relaxed);
        report.push_str(&format!(
            "\n📞 Callbacks: Input={}, Output={}\n",
            input_callbacks, output_callbacks
        ));

        // Recent anomalies
        if !anomalies.is_empty() {
            report.push_str("\n⚠️  Recent Anomalies:\n");
            for anomaly in anomalies.iter().rev().take(10) {
                report.push_str(&format!(
                    "  {} - {}: {:.2}\n",
                    anomaly.stage, anomaly.issue, anomaly.value
                ));
            }
        }

        report
    }

    pub fn sanity_test_codec(&self) -> Result<(), String> {
        info!("🧪 Running codec sanity test...");

        // Generate test signal
        let test_samples = TestSignal::Sine1kHz.generate(480, 48000);
        let original_fp = AudioFingerprint::from_samples(&test_samples);

        info!(
            "  Original: RMS={:.3}, Peak={:.3}, ZC={}",
            original_fp.rms, original_fp.peak, original_fp.zero_crossings
        );

        if original_fp.is_silent() {
            return Err("Test signal is silent!".to_string());
        }

        Ok(())
    }
}

// ============================================================================
// Helper function for periodic diagnostic checks
// ============================================================================

pub fn run_diagnostic_checks(diagnostics: &PipelineDiagnostics) {
    if !diagnostics.is_enabled() {
        return;
    }

    // Check ring buffer drift
    if let Some(anomaly) = diagnostics.input_ring.check_rate_mismatch() {
        diagnostics.record_anomaly(anomaly);
    }

    if let Some(anomaly) = diagnostics.output_ring.check_rate_mismatch() {
        diagnostics.record_anomaly(anomaly);
    }

    // Log report every 2 seconds
    static LAST_REPORT: AtomicU64 = AtomicU64::new(0);
    let now = Instant::now().elapsed().as_secs();
    let last = LAST_REPORT.load(Ordering::Relaxed);

    if now - last >= 2 {
        LAST_REPORT.store(now, Ordering::Relaxed);
        info!("\n{}", diagnostics.get_report());
    }
}

