### Borrowing & Ownership
- Prefer `&T` over `.clone()` unless ownership transfer is required
- Use `&str` over `String`, `&[T]` over `Vec<T>` in function parameters
- Small `Copy` types (≤24 bytes) can be passed by value
- Use `Cow<'_, T>` when ownership is ambiguous

### Error Handling
- Return `Result<T, E>` for fallible operations; avoid `panic!` in production
- Never use `unwrap()`/`expect()` outside tests
- Use `thiserror` for library errors, `anyhow` for binaries only
- Prefer `?` operator over match chains for error propagation

### Performance
- Always benchmark with `--release` flag
- Run `cargo clippy -- -D clippy::perf` for performance hints
- Avoid cloning in loops; use `.iter()` instead of `.into_iter()` for Copy types
- Prefer iterators over manual loops; avoid intermediate `.collect()` calls

### Linting
Run regularly: `cargo clippy --all-targets --all-features --locked -- -D warnings`

Key lints to watch:
- `redundant_clone` - unnecessary cloning
- `large_enum_variant` - oversized variants (consider boxing)
- `needless_collect` - premature collection

Use `#[expect(clippy::lint)]` over `#[allow(...)]` with justification comment.

### Testing
- Name tests descriptively: `process_should_return_error_when_input_empty()`
- One assertion per test when possible
- Use doc tests (`///`) for public API examples
- Consider `cargo insta` for snapshot testing generated output

### Generics & Dispatch
- Prefer generics (static dispatch) for performance-critical code
- Use `dyn Trait` only when heterogeneous collections are needed
- Box at API boundaries, not internally

### Type State Pattern
Encode valid states in the type system to catch invalid operations at compile time:
```rust
struct Connection { /* ... */ _state: PhantomData }
struct Disconnected;
struct Connected;

impl Connection {
    fn send(&self, data: &[u8]) { /* only connected can send */ }
}
```

### Documentation
- `//` comments explain *why* (safety, workarounds, design rationale)
- `///` doc comments explain *what* and *how* for public APIs
- Every `TODO` needs a linked issue: `// TODO(#42): ...`
- Enable `#![deny(missing_docs)]` for libraries

### Application & UI Code
- Prefer flat, linear layout code in examples and binaries — don't extract into
  builder functions that return tuples just to shorten `main()`.
- Extract only when a function has **its own state** (timers, event loops) or is
  **reused across multiple call sites**.
- Use structs to bundle related handles that move into closures together,
  not to decompose a naturally sequential flow.
- Callbacks should sit close to the widgets they control, not in separate wiring functions.
- A long but linear `main()` that reads top-to-bottom is better than a short
  `main()` that forces the reader to jump between builder functions and tuple unpacking.

### FLTK Patterns
**Widget handles are lightweight clones.** `frame::Frame`, `button::Button`, etc. are
cheap handle types (like `Rc` internally). Clone freely to pass into closures — never
wrap in `Rc<RefCell<Frame>>` when a plain `.clone()` works.

**`app::Sender` is `Copy`.** No need for `let s = sender; ... let s = sender;` rebinding
chains — just use `sender` directly in each closure. It copies implicitly.

**Popups — prefer non-modal over modal:**
- Modal loops (`while popup.shown() { app::wait(); }`) hijack the FLTK event loop.
  Network messages, voice events, and UI updates freeze until the popup closes.
  This causes chat and sidebar to stop refreshing.
- **Prefer non-modal windows:** call `wind.show()` and return. The popup coexists
  with the main window, and `App::run()` keeps processing messages normally.
- Track open popups with `Option<window::Window>` on the owning struct. Close the
  existing one before opening a new one to prevent duplicates.
- Since closures inside widget callbacks can't call `&mut self` methods (borrow
  conflict), route popup-open requests through the message system: callback sends
  `UiMessage::OpenFoo`, `App` handles it by calling `sidebar.open_foo()`.

**`Event::Unfocus` for closing non-modal popups:**
- Works reliably for non-modal windows (main event loop is running).
- Unreliable for modal popups (fires before child callbacks, swallowing clicks).
- For borderless popups, `Unfocus` is the cleanest close-on-click-outside mechanism.

**`WidgetExt::inside()` takes a widget reference, NOT coordinates.** For hit-testing
mouse events against a window's bounds, compare `event_x_root()`/`event_y_root()`
against `widget.x()`/`widget.y()`/`widget.w()`/`widget.h()` manually.

**Event coordinates go stale after the callback returns.** `app::event_x_root()` /
`event_y_root()` are only valid during the FLTK event handler. If you route actions
through a message channel (e.g. `UiMessage`), capture the coordinates at click time
and pass them through the message. By the time the message is processed, the event
state has been overwritten.

**`widget.hide()` / `widget.show()` removes/adds from layout.** Calling `hide()` on
a widget inside a `Flex` removes it from layout calculations. `show()` adds it back
but the `Flex` may not re-layout properly, leaving the widget invisible. Instead,
make widgets "invisible" by setting their label and background colors to match the
parent — always present in layout but visually hidden. Toggle colors on hover to reveal.

**Hover detection on container widgets:**
- `Event::Enter`/`Event::Leave` on a `Flex` may not fire when child widgets cover
  the surface — the mouse enters the child, not the parent.
- Use `Event::Enter | Event::Move` together for reliable hover detection. Guard with
  a `Rc<RefCell<bool>>` flag so the hover-on logic runs once, not every pixel.
- On `Event::Leave`, verify the mouse truly left the row bounds (FLTK fires `Leave`
  when moving between child widgets within the same parent).

**Incremental list updates — avoid full rebuilds:**
- Calling `pack.clear()` + rebuild on every state change destroys scroll position,
  causes flickering, and resets hover state.
- Store widget handles (indicator, label, container) alongside each item ID.
- On update, compare the current ID set against existing rows. Same set → update
  labels/colors in-place. Different set → full rebuild.
- Only `pack.clear()` when items are actually added or removed.

**Callbacks in FLTK:**
- `set_callback` on `button::Button` is reliable everywhere.
- `set_callback` on `button::CheckButton` and `handle()` on `frame::Frame` can be
  unreliable inside modal popup loops. Prefer real `Button` widgets for clickable actions.
- `slider.handle()` can catch `Event::Released` to detect when the user finishes
  dragging — useful for commit-on-release patterns.

**Closures capture state at build time.** Values like `current_volume` or `is_muted`
captured in a closure reflect the state when the closure was created, not when it runs.
For fresh state, either: (a) read from shared atomics/Rc in the closure, or (b) route
through the message system and have the handler read current state from the app's
authoritative data.

**macOS Cocoa pitfalls:**
- `group::Pack`: resize the pack to the correct dimensions *before* adding children via
  `.add()`. On macOS, children added to an unsized pack get zero-size bounding boxes.
- Don't wrap `Flex::default()` inside `begin()`/`end()` when using `.add()` — causes
  double-parenting on macOS, corrupting the widget tree.

**Timer pattern:**
- Use `app::add_timeout3` + `app::repeat_timeout3` for periodic updates.
- Bundle all widget handles the timer needs into a struct and move it into the closure.
- Keep timer closures focused on reading state and updating labels — don't put business
  logic in timers.