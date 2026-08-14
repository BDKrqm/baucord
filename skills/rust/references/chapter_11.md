---
name: rust-refactoring
description: Safe refactoring of Rust codebases — restructuring code for clarity without changing behavior. Use when the user wants to clean up, reorganize, extract modules, reduce complexity, or improve structure in Rust code. Covers borrow checker pitfalls, thread/closure extraction patterns, channel ownership, and atomic state management.
---

# Rust Refactoring Skill

Safe structural refactoring for Rust projects. The goal is always: **improve organization and readability without changing behavior.**

## Core Principles

1. **No behavioral changes during refactoring.** If it compiled and worked before, it must compile and work identically after. Never "improve" logic while restructuring — those are separate PRs.

2. **The borrow checker is not optional.** Every refactoring must be designed around Rust's ownership model. A refactoring that looks clean in pseudocode but fights the borrow checker is a bad refactoring.

3. **Refactor in layers.** Do the safest, most mechanical changes first (renames, inline format args, unused import removal). Then structural changes (extract modules, convert closures to structs). Then ownership changes (channel restructuring, atomic consolidation). Each layer should compile before moving to the next.

---

## Refactoring Patterns

### Pattern 1: Extract Thread Closures into Structs

**When to use:** A `thread::spawn(move || { ... })` closure captures 5+ variables and contains 50+ lines of logic.

**Before (anti-pattern):**

```rust
fn start_worker(&mut self) {
    let rx = self.rx.clone();
    let tx = self.tx.clone();
    let stats = self.stats.clone();
    let flag_a = self.flag_a.clone();
    let flag_b = self.flag_b.clone();
    let config_val = self.config_val.clone();

    self.handle = Some(thread::spawn(move || {
        let mut state_a = 0;
        let mut state_b = HashMap::new();
        // ... 200 lines of logic using all captured variables ...
    }));
}
```

**After:**

```rust
struct Worker {
    rx: Receiver<T>,
    tx: Sender<U>,
    stats: Arc<Stats>,
    flag_a: Arc<AtomicBool>,
    flag_b: Arc<AtomicBool>,
    state_a: u32,
    state_b: HashMap<K, V>,
}

impl Worker {
    fn new(rx, tx, stats, flag_a, flag_b) -> Self { ... }
    fn run(&mut self) { /* clean loop calling self.process_item() */ }
    fn process_item(&mut self, item: T) { ... }
}

// In the spawning code:
pub fn spawn(rx, tx, stats, flag_a, flag_b) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut worker = Worker::new(rx, tx, stats, flag_a, flag_b);
        worker.run();
    })
}
```

**Benefits:** Named fields instead of captured variables. Logic is methods, not a monolithic closure. Testable in isolation.

**⚠️ CRITICAL — Borrow Checker Trap:**

When thread state lives in a struct, calling `self.method()` borrows ALL of `self`. If you also hold a `&mut` reference to a field (e.g., from `self.map.entry().or_insert()`), you cannot call any `self.method()` until that reference is dropped.

**This breaks:**

```rust
fn run(&mut self) {
    let entry = self.decoders.entry(key).or_insert_with(|| make_new());
    self.process(entry);  // ERROR: self already borrowed via self.decoders
}
```

**Solutions (pick one):**

**(a) Keep it inline.** If the logic needs sequential access to a mutable map entry AND other fields, keep it as one method. Add a comment explaining why:

```rust
/// Intentionally one method — splitting would fight the borrow checker
/// because the decoder reference from self.decoders must stay alive
/// across PLC → FEC → normal decode (stateful sequence).
fn process_packet(&mut self, packet: Packet) {
    let decoder = self.decoders.entry(ssrc).or_insert_with(make_new);
    // ... all logic inline using `decoder` and `self.other_field` ...
}
```

**(b) Drop the borrow first.** If you can split into phases:

```rust
fn run(&mut self) {
    // Phase 1: ensure entry exists (borrow released at semicolon)
    self.decoders.entry(key).or_insert_with(|| make_new());

    // Phase 2: now safe to call methods
    self.handle_gaps(key);
    self.decode_current(key);
}

fn handle_gaps(&mut self, key: K) {
    let decoder = self.decoders.get_mut(&key).unwrap();
    // ... use decoder + self.other_fields ...
}
```

**⚠️ Only works if each method's decoder usage is self-contained.** If method A mutates the decoder state and method B depends on that exact state, and both need the same `&mut Decoder`, they must stay in one method or the decoder state sequence may break.

**(c) Split the struct.** Move the map into a separate struct that doesn't overlap:

```rust
struct DecoderSet { decoders: HashMap<K, Decoder>, last_seq: HashMap<K, u32> }
struct Worker { decoder_set: DecoderSet, tx: Sender<T>, stats: Arc<Stats> }
```

Now `self.decoder_set.process()` doesn't borrow `self.tx` or `self.stats`.

### Pattern 2: Channel Ownership Cleanup

**When to use:** A struct stores `(Sender<T>, Receiver<T>)` tuples and accesses them with `.0` / `.1`.

**Before:**

```rust
struct Engine {
    audio_to_encoder: (Sender<Vec<f32>>, Receiver<Vec<f32>>),
    encoder_to_network: (Sender<AudioPacket>, Receiver<AudioPacket>),
}
// Usage: self.audio_to_encoder.0.send(...)
```

**After:**

```rust
struct Engine {
    /// Input worker sends frames here for encoding.
    audio_to_encoder_tx: Sender<Vec<f32>>,
    /// Voice thread polls encoded packets from here.
    encoder_to_network_rx: Receiver<AudioPacket>,
}
```

**Rules:**

- Name each half by its role: `_tx` suffix for senders, `_rx` for receivers.
- Only store the half you actually use. Pass the other half to the consumer at construction time.
- If a channel is created in `new()` but one end is passed to a thread in `start()`, pass it during thread spawn and don't store it on the struct.

### Pattern 3: Atomic Handle Consolidation

**When to use:** A parent struct creates an `Arc<AtomicBool>`, passes it to a child, then reads back a "handle" from the child — resulting in stale references until the child is started.

**Before:**

```rust
struct Engine {
    output_gain: Arc<AtomicU32>,  // Created in new()
}

fn start_output(&mut self) {
    let output = AudioOutput::new(..., self.config.output_gain);
    self.output_gain = output.output_gain_handle();  // REASSIGNMENT — stale until here
    self.output = Some(output);
}
```

**After:** Create the atomic once in the parent, pass it down:

```rust
fn new(config) -> Self {
    let output_gain = Arc::new(AtomicU32::new(config.output_gain.to_bits()));
    Self { output_gain, ... }
}

fn start_output(&mut self) {
    let output = AudioOutput::new(..., self.output_gain.clone());
    // No reassignment needed — same Arc
}
```

### Pattern 4: Two-Phase Init Consolidation

**When to use:** `new()` creates partial state, `start()` spawns threads. Threads that don't depend on `start()`-phase resources (like audio devices) can be spawned eagerly.

**Before:**

```rust
fn new() -> Self { /* channels only */ }
fn start(&mut self) {
    self.start_encoder_thread();   // No device dependency
    self.start_decoder_pool();     // No device dependency
    self.start_input()?;           // Needs audio device
    self.start_output()?;          // Needs audio device
}
```

**After:**

```rust
fn new() -> Self {
    let encoder_thread = encoder::spawn(rx, tx, stats);
    let decoder_pool = decoder::spawn_pool(rx, tx, stats, stop);
    Self { encoder_thread, decoder_pool, ... }
}
fn start(&mut self) {
    self.start_input()?;   // Only device-dependent work
    self.start_output()?;
}
```

**Benefit:** Fewer partial-init states. If `start()` fails on the output device, the encoder/decoder threads are already running and will cleanly shut down via channel disconnection.

---

## Safe Cleanup Passes

These are mechanical, low-risk changes to do first before structural refactoring:

### Inline Format Args

```rust
// Before
format!("Failed: {}", e)
// After
format!("Failed: {e}")
```

### Remove Unused Imports

Run `cargo check` and fix all `unused_imports` warnings before making structural changes. This prevents confusion about which imports are needed by the new structure.

### Unused Dependencies in Cargo.toml

Check if crate dependencies are actually used. Common culprits:

- Meta-crates like `crossbeam` when only `crossbeam-channel` is used
- Crates that were used in deleted code (`rand`, `chrono`, `dirs`)

### Clippy Lints

Fix clippy warnings (unnecessary nested `if`, redundant patterns) as a separate pass.

### Add `Drop` Impls for Thread Handles

If a struct owns a `JoinHandle` and has a `shutdown()` method, add a `Drop` impl that calls it. Prevents thread leaks if the handle is dropped without explicit shutdown:

```rust
impl Drop for ThreadHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}
```

---

## State Machine Refactoring (Mute/Deafen Example)

When two boolean states interact (e.g., mute and deafen where deafen implies mute), track the user's _intent_ separately from the _effective_ state:

```rust
struct State {
    /// Explicit mute set by the user (independent of deafen).
    user_muted: bool,
}

impl State {
    /// Whether the mic should be muted — true if either user-muted or deafened.
    fn effective_mute(&self) -> bool {
        self.user_muted || self.is_deafened()
    }

    fn set_muted(&mut self, muted: bool) {
        self.user_muted = muted;
        self.apply_effective_mute();
    }

    fn set_deafened(&mut self, deafened: bool) {
        self.engine.set_deafened(deafened);
        // Recompute — undeafen restores the user's explicit mute state
        self.apply_effective_mute();
    }
}
```

**Bug this prevents:** Without tracking `user_muted` separately, undeafening always unmutes — even if the user had explicitly muted before deafening.

---

## Module Extraction Checklist

When extracting a chunk of code into its own module:

1. **Identify the boundary.** What data does this code own? What does it borrow? What channels does it read/write?
2. **Define the public API.** Usually one `spawn()` function and a handle struct.
3. **Move the code.** Create the new file, move the struct + impl.
4. **Fix visibility.** The new module needs `pub(crate)` or `pub` on types the parent uses. Internal helpers stay private.
5. **Update `mod` declarations** in `lib.rs` or `main.rs`.
6. **Compile.** Fix errors before making any further changes.
7. **Test.** The behavior must be identical.

---

## Anti-Patterns to Avoid

### Don't split stateful sequences across methods that re-borrow

If steps A → B → C must happen on the same `&mut T` in order (e.g., Opus PLC → FEC → decode), keep them in one method. The borrow checker will fight you if you split them, and even if you work around it, you risk reordering stateful mutations.

### Don't refactor and change behavior simultaneously

"While I'm in here, let me also improve the FEC recovery..." — NO. Refactor first, ship it, verify it works identically, THEN change behavior in a separate commit.

### Don't store channel halves you don't use

If a struct creates `bounded(8)` and immediately passes the `Receiver` to a spawned thread, don't store the `Receiver` on the struct. It wastes a field and confuses readers about who owns what.

### Don't over-decompose into methods that fight the borrow checker

Sometimes one 80-line method is better than four 20-line methods that each need `self.map.get_mut()`. The borrow checker is telling you the data flow is sequential and coupled — listen to it.
