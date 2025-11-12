// ui/diagnostics.rs - Refactored to use message passing
use crate::ui::messages::UiMessage;
use crate::ui::theme::DarkTheme;
use chrono::Local;
use fltk::enums::Color;
use fltk::{prelude::*, *};
use std::collections::VecDeque;

pub struct DiagnosticsWindow;

impl DiagnosticsWindow {
    pub fn show(sender: app::Sender<UiMessage>) {
        let mut wind = window::Window::default()
            .with_size(800, 600)
            .with_label("Voice Diagnostics Terminal");
        wind.make_resizable(true);
        wind.set_color(DarkTheme::BG_PRIMARY);

        let mut main_col = group::Flex::default_fill().column();
        main_col.set_margins(10, 10, 10, 10);
        main_col.set_spacing(10);

        // Title bar
        let title_row = group::Flex::default().row();
        main_col.fixed(&title_row, 40);

        let mut title = frame::Frame::default()
            .with_label("🔧 Voice Diagnostics Terminal")
            .with_align(enums::Align::Left | enums::Align::Inside);
        title.set_label_size(16);
        title.set_label_color(DarkTheme::TEXT_PRIMARY);

        let mut help_text = frame::Frame::default()
            .with_label("Type 'help' for commands • ↑↓ for history • Tab for autocomplete")
            .with_align(enums::Align::Right | enums::Align::Inside);
        help_text.set_label_size(11);
        help_text.set_label_color(DarkTheme::TEXT_MUTED);

        title_row.end();

        // Terminal output display
        let mut output_display = text::TextDisplay::default();
        let mut output_buffer = text::TextBuffer::default();
        output_display.set_buffer(output_buffer.clone());
        output_display.set_color(Color::from_rgb(16, 16, 24));
        output_display.set_text_color(Color::from_rgb(0, 255, 0));
        output_display.set_text_font(enums::Font::Courier);
        output_display.set_text_size(12);
        output_display.wrap_mode(text::WrapMode::AtBounds, 0);

        // Input area
        let mut input_row = group::Flex::default().row();
        main_col.fixed(&input_row, 35);

        let mut prompt = frame::Frame::default().with_label(">");
        prompt.set_label_color(Color::from_rgb(0, 255, 0));
        prompt.set_label_font(enums::Font::CourierBold);
        prompt.set_label_size(14);
        input_row.fixed(&prompt, 20);

        let mut command_input = input::Input::default();
        command_input.set_color(Color::from_rgb(16, 16, 24));
        command_input.set_text_color(Color::from_rgb(0, 255, 0));
        command_input.set_text_font(enums::Font::Courier);
        command_input.set_text_size(12);
        command_input.set_frame(enums::FrameType::BorderBox);

        input_row.end();

        // Button row
        let mut button_row = group::Flex::default().row();
        main_col.fixed(&button_row, 30);
        button_row.set_spacing(10);

        let mut clear_btn = button::Button::default().with_label("Clear");
        clear_btn.set_color(DarkTheme::BG_TERTIARY);
        clear_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        clear_btn.set_frame(enums::FrameType::FlatBox);
        button_row.fixed(&clear_btn, 80);

        let mut export_btn = button::Button::default().with_label("Export Log");
        export_btn.set_color(DarkTheme::BG_TERTIARY);
        export_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        export_btn.set_frame(enums::FrameType::FlatBox);
        button_row.fixed(&export_btn, 100);

        let mut auto_scroll = button::CheckButton::default().with_label("Auto-scroll");
        auto_scroll.set_checked(true);
        auto_scroll.set_color(DarkTheme::BG_PRIMARY);
        auto_scroll.set_label_color(DarkTheme::TEXT_PRIMARY);
        auto_scroll.set_selection_color(DarkTheme::ACCENT);
        button_row.fixed(&auto_scroll, 100);

        button_row.add(&frame::Frame::default()); // Spacer

        let mut close_btn = button::Button::default().with_label("Close");
        close_btn.set_color(DarkTheme::BG_TERTIARY);
        close_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        close_btn.set_frame(enums::FrameType::FlatBox);
        button_row.fixed(&close_btn, 80);

        button_row.end();
        main_col.end();
        wind.end();
        wind.show();
        let s = sender;
close_btn.set_callback(move |_| {
    s.send(UiMessage::DiagnosticsWindowClosed);
    wind.hide();
});
// // Handle window close (X button)
// let s_close = sender.clone();
// wind.set_callback(move |w| {
//     if app::event() == enums::Event::Close {
//         s_close.send(UiMessage::DiagnosticsWindowClosed);
//         w.hide();
//     }
// });



        // Initial welcome message
        output_buffer.append(&"=".repeat(60));
        output_buffer.append("\n");
        output_buffer.append("Voice Diagnostics Terminal v1.0\n");
        output_buffer.append("Type 'help' for available commands\n");
        output_buffer.append(&"=".repeat(60));
        output_buffer.append("\n\n");

        // Command history
        let mut command_history: VecDeque<String> = VecDeque::with_capacity(50);
        let mut history_index = 0usize;

        // Setup command input handler
        let mut output_buffer_cmd = output_buffer.clone();
        let mut display_clone = output_display.clone();
        let auto_scroll_check = auto_scroll.clone();
        let s = sender;

        command_input.handle(move |input, event| match event {
            enums::Event::KeyDown => match app::event_key() {
                enums::Key::Enter => {
                    let cmd = input.value();
                    if !cmd.trim().is_empty() {
                        // Add to history
                        command_history.push_back(cmd.clone());
                        if command_history.len() > 50 {
                            command_history.pop_front();
                        }
                        history_index = command_history.len();

                        // Show command in output
                        let timestamp = Local::now().format("%H:%M:%S");
                        output_buffer_cmd.append(&format!("[{}] > {}\n", timestamp, cmd));

                        // Process command locally or send message
                        let response = Self::process_command(&cmd, &s);
                        output_buffer_cmd.append(&format!("{}\n", response));

                        // Auto-scroll if enabled
                        if auto_scroll_check.is_checked() {
                            let length = output_buffer_cmd.length();
                            display_clone.set_insert_position(length);
                            display_clone
                                .scroll(display_clone.count_lines(0, length, true), 0);
                        }

                        input.set_value("");
                    }
                    true
                }
                enums::Key::Up => {
                    // History navigation
                    if !command_history.is_empty() && history_index > 0 {
                        history_index -= 1;
                        if let Some(cmd) = command_history.get(history_index) {
                            input.set_value(cmd);
                        }
                    }
                    true
                }
                enums::Key::Down => {
                    // History navigation
                    if history_index < command_history.len() {
                        history_index += 1;
                        if history_index >= command_history.len() {
                            input.set_value("");
                        } else if let Some(cmd) = command_history.get(history_index) {
                            input.set_value(cmd);
                        }
                    }
                    true
                }
                enums::Key::Tab => {
                    // Autocomplete
                    let current = input.value();
                    if let Some(completed) = Self::autocomplete_command(&current) {
                        input.set_value(&completed);
                    }
                    true
                }
                _ => false,
            },
            _ => false,
        });

        // Clear button
        let mut output_buffer_clear = output_buffer.clone();
        clear_btn.set_callback(move |_| {
            output_buffer_clear.set_text("");
        });

        // Export button
        let output_buffer_export = output_buffer.clone();
        export_btn.set_callback(move |_| {
            let content = output_buffer_export.text();
            let filename = format!(
                "diagnostics_{}.txt",
                Local::now().format("%Y%m%d_%H%M%S")
            );
            if let Ok(mut file) = std::fs::File::create(&filename) {
                use std::io::Write;
                let _ = write!(file, "{}", content);
                println!("Exported diagnostics to {}", filename);
            }
        });

        
    }

    fn process_command(cmd: &str, sender: &app::Sender<UiMessage>) -> String {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return String::new();
        }

        match parts[0].to_lowercase().as_str() {
            "help" | "?" => Self::get_help_text(),
            "clear" => "Screen cleared".to_string(),
            "status" => {
                sender.send(UiMessage::RunDiagnosticCommand("status".to_string()));
                "Requesting status...".to_string()
            }
            "stats" => {
                sender.send(UiMessage::RunDiagnosticCommand("stats".to_string()));
                "Requesting statistics...".to_string()
            }
            "devices" => {
                sender.send(UiMessage::RunDiagnosticCommand("devices".to_string()));
                "Requesting device list...".to_string()
            }
            "diag" => {
                if parts.len() < 2 {
                    "Usage: diag [on|off|report]".to_string()
                } else {
                    let cmd = format!("diag {}", parts[1]);
                    sender.send(UiMessage::RunDiagnosticCommand(cmd));
                    format!("Sent: diag {}", parts[1])
                }
            }
            "test" => {
                if parts.len() < 2 {
                    "Usage: test [off|sine|chirp|noise]".to_string()
                } else {
                    let cmd = format!("test {}", parts[1]);
                    sender.send(UiMessage::RunDiagnosticCommand(cmd));
                    format!("Test mode: {}", parts[1])
                }
            }
            "loopback" => {
                if parts.len() < 2 {
                    "Usage: loopback [on|off]".to_string()
                } else {
                    match parts[1] {
                        "on" => {
                            sender.send(UiMessage::SetLoopback(true));
                            "✓ Loopback enabled".to_string()
                        }
                        "off" => {
                            sender.send(UiMessage::SetLoopback(false));
                            "✓ Loopback disabled".to_string()
                        }
                        _ => "Usage: loopback [on|off]".to_string(),
                    }
                }
            }
            "set" => {
                if parts.len() < 3 {
                    "Usage: set [input|output] <device_name>".to_string()
                } else {
                    let device_name = parts[2..].join(" ");
                    match parts[1] {
                        "input" => {
                            sender.send(UiMessage::SetInputDevice(Some(device_name.clone())));
                            format!("Setting input device to: {}", device_name)
                        }
                        "output" => {
                            sender.send(UiMessage::SetOutputDevice(Some(device_name.clone())));
                            format!("Setting output device to: {}", device_name)
                        }
                        _ => "Usage: set [input|output] <device_name>".to_string(),
                    }
                }
            }
            _ => format!(
                "Unknown command: '{}'. Type 'help' for available commands.",
                parts[0]
            ),
        }
    }

    fn get_help_text() -> String {
        vec![
            "Available Commands:",
            "─────────────────",
            "help, ?           - Show this help",
            "clear            - Clear terminal",
            "status           - Show connection status",
            "stats            - Show voice statistics",
            "devices          - List audio devices",
            "report           - Show diagnostic report",
            "",
            "Diagnostics:",
            "diag on          - Enable diagnostics",
            "diag off         - Disable diagnostics",
            "diag report      - Show detailed report",
            "",
            "Audio Tests:",
            "test off         - Disable test mode",
            "test sine        - 1kHz test tone",
            "test chirp       - Frequency sweep",
            "test noise       - White noise",
            "",
            "Loopback:",
            "loopback on      - Enable audio loopback",
            "loopback off     - Disable audio loopback",
            "",
            "Device Control:",
            "set input <name> - Change input device",
            "set output <name>- Change output device",
        ]
        .join("\n")
    }

    fn autocomplete_command(partial: &str) -> Option<String> {
        let commands = vec![
            "help",
            "clear",
            "status",
            "stats",
            "devices",
            "report",
            "diag on",
            "diag off",
            "diag report",
            "test off",
            "test sine",
            "test chirp",
            "test noise",
            "loopback on",
            "loopback off",
            "set input",
            "set output",
        ];

        for cmd in commands {
            if cmd.starts_with(partial) && cmd != partial {
                return Some(cmd.to_string());
            }
        }
        None
    }
}