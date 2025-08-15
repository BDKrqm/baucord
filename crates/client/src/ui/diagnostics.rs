use crate::ui::theme::DarkTheme;
use fltk::enums::Color;
use crate::voice::VoiceChat;
use crate::voice::diagnostics::TestMode;
use fltk::{prelude::*, *};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use chrono::Local;

pub struct DiagnosticsWindow {
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    command_history: VecDeque<String>,
    history_index: usize,
}

impl DiagnosticsWindow {
    pub fn new(voice_chat: Arc<Mutex<Option<VoiceChat>>>) -> Self {
        Self {
            voice_chat,
            command_history: VecDeque::with_capacity(50),
            history_index: 0,
        }
    }

    pub fn show_window(&mut self) {
        let mut wind = window::Window::default()
            .with_size(800, 600)
            .with_label("Voice Diagnostics Terminal");
        wind.make_resizable(true);
        wind.set_color(DarkTheme::BG_PRIMARY);

        let mut main_col = group::Flex::default_fill().column();
        main_col.set_margins(10, 10, 10, 10);
        main_col.set_spacing(10);

        // Title bar
        let mut title_row = group::Flex::default().row();
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
        output_display.set_color(Color::from_rgb(16, 16, 24)); // Darker terminal background
        output_display.set_text_color(Color::from_rgb(0, 255, 0)); // Classic terminal green
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

        // Initial welcome message
        self.append_output(&mut output_buffer, "=".repeat(60).as_str());
        self.append_output(&mut output_buffer, "Voice Diagnostics Terminal v1.0");
        self.append_output(&mut output_buffer, "Type 'help' for available commands");
        self.append_output(&mut output_buffer, "=".repeat(60).as_str());
        self.append_output(&mut output_buffer, "");

        // Auto-scroll to bottom
        let mut scroll_to_bottom = {
            let display = output_display.clone();
            let buffer = output_buffer.clone();
            move || {
                let length = buffer.length();
                display.clone().set_insert_position(length);
                display.clone().scroll(display.count_lines(0, length, true), 0);
            }
        };

        // Setup command processing
        let voice_chat = self.voice_chat.clone();
        let mut output_buffer_cmd = output_buffer.clone();
        let mut command_history = self.command_history.clone();
        let mut history_index = self.history_index;
        let mut display_clone = output_display.clone();
        let auto_scroll_check = auto_scroll.clone();

        command_input.handle(move |input, event| {
            match event {
                enums::Event::KeyDown => {
                    match app::event_key() {
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

                                // Process command
                                let response = process_command(&cmd, &voice_chat);
                                output_buffer_cmd.append(&format!("{}\n", response));

                                // Auto-scroll if enabled
                                if auto_scroll_check.is_checked() {
                                    let length = output_buffer_cmd.length();
                                    display_clone.set_insert_position(length);
                                    display_clone.scroll(display_clone.count_lines(0, length, true), 0);
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
                            // Simple autocomplete
                            let current = input.value();
                            if let Some(completed) = autocomplete_command(&current) {
                                input.set_value(&completed);
                            }
                            true
                        }
                        _ => false
                    }
                }
                _ => false
            }
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
            let filename = format!("diagnostics_{}.txt", Local::now().format("%Y%m%d_%H%M%S"));
            if let Ok(mut file) = std::fs::File::create(&filename) {
                use std::io::Write;
                let _ = write!(file, "{}", content);
                println!("Exported diagnostics to {}", filename);
            }
        });

        // Close button
        close_btn.set_callback(move |_| {
            wind.hide();
        });

        
    }

    fn append_output(&self, buffer: &mut text::TextBuffer, text: &str) {
        buffer.append(&format!("{}\n", text));
    }
}

fn process_command(cmd: &str, voice_chat: &Arc<Mutex<Option<VoiceChat>>>) -> String {
    let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }

    match parts[0].to_lowercase().as_str() {
        "help" | "?" => {
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
            ].join("\n")
        }
        
        "status" => {
            if let Ok(vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    format!("Voice System: ACTIVE\nSpeaking: {}\n{}", 
                        if vc.is_speaking() { "Yes" } else { "No" },
                        vc.get_device_info())
                } else {
                    "Voice System: NOT INITIALIZED".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "stats" => {
            if let Ok(vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    vc.get_stats()
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "devices" => {
            if let Ok(vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    let mut result = String::new();
                    
                    result.push_str("INPUT DEVICES:\n");
                    if let Ok(inputs) = vc.get_input_devices() {
                        for (name, valid) in inputs {
                            result.push_str(&format!("  {} {}\n", 
                                if valid { "✓" } else { "✗" }, name));
                        }
                    }
                    
                    result.push_str("\nOUTPUT DEVICES:\n");
                    if let Ok(outputs) = vc.get_output_devices() {
                        for (name, valid) in outputs {
                            result.push_str(&format!("  {} {}\n", 
                                if valid { "✓" } else { "✗" }, name));
                        }
                    }
                    
                    result
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "diag" => {
            if parts.len() < 2 {
                return "Usage: diag [on|off|report]".to_string();
            }
            
            if let Ok(mut vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    match parts[1] {
                        "on" => {
                            vc.enable_diagnostics(true);
                            "✓ Diagnostics enabled - auto-updates every 2 seconds".to_string()
                        }
                        "off" => {
                            vc.enable_diagnostics(false);
                            "✓ Diagnostics disabled".to_string()
                        }
                        "report" => {
                            vc.get_diagnostic_report()
                        }
                        _ => format!("Unknown option: {}", parts[1])
                    }
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "report" => {
            if let Ok(vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    vc.get_diagnostic_report()
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "test" => {
            if parts.len() < 2 {
                return "Usage: test [off|sine|chirp|noise]".to_string();
            }
            
            if let Ok(mut vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    match parts[1] {
                        "off" => {
                            vc.set_diagnostic_test_mode(TestMode::Disabled);
                            "✓ Test mode disabled".to_string()
                        }
                        "sine" => {
                            vc.set_diagnostic_test_mode(TestMode::Sine1kHz);
                            "✓ Playing 1kHz sine wave".to_string()
                        }
                        "chirp" => {
                            vc.set_diagnostic_test_mode(TestMode::Chirp);
                            "✓ Playing frequency sweep (200Hz-2.2kHz)".to_string()
                        }
                        "noise" => {
                            vc.set_diagnostic_test_mode(TestMode::WhiteNoise);
                            "✓ Playing white noise".to_string()
                        }
                        _ => format!("Unknown test mode: {}", parts[1])
                    }
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "loopback" => {
            if parts.len() < 2 {
                return "Usage: loopback [on|off]".to_string();
            }
            
            if let Ok(mut vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    match parts[1] {
                        "on" => {
                            vc.set_loopback(true);
                            "✓ Loopback enabled - you should hear yourself".to_string()
                        }
                        "off" => {
                            vc.set_loopback(false);
                            "✓ Loopback disabled".to_string()
                        }
                        _ => format!("Usage: loopback [on|off]")
                    }
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "set" => {
            if parts.len() < 3 {
                return "Usage: set [input|output] <device_name>".to_string();
            }
            
            if let Ok(mut vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    let device_name = parts[2..].join(" ");
                    match parts[1] {
                        "input" => {
                            match vc.set_input_device(Some(&device_name)) {
                                Ok(_) => format!("✓ Input device changed to: {}", device_name),
                                Err(e) => format!("✗ Failed to change input device: {}", e)
                            }
                        }
                        "output" => {
                            match vc.set_output_device(Some(&device_name)) {
                                Ok(_) => format!("✓ Output device changed to: {}", device_name),
                                Err(e) => format!("✗ Failed to change output device: {}", e)
                            }
                        }
                        _ => "Usage: set [input|output] <device_name>".to_string()
                    }
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "debug" => {
            if let Ok(vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    // Get detailed internal state
                    let mut result = String::new();
                    result.push_str("=== DEBUG INFO ===\n");
                    result.push_str(&format!("Speaking: {}\n", vc.is_speaking()));
                    result.push_str(&format!("Loopback: {}\n", vc.is_loopback_enabled()));
                    result.push_str(&format!("\n{}\n", vc.get_stats()));
                    result.push_str(&format!("\n{}\n", vc.get_diagnostic_report()));
                    result
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        "reset" => {
            if let Ok(mut vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    // Reset all counters and clear buffers
                    vc.reset_diagnostics();
                    "✓ Diagnostics reset".to_string()
                } else {
                    "Voice system not initialized".to_string()
                }
            } else {
                "Error: Could not access voice system".to_string()
            }
        }
        
        _ => {
            format!("Unknown command: '{}'. Type 'help' for available commands.", parts[0])
        }
    }
}

fn autocomplete_command(partial: &str) -> Option<String> {
    let commands = vec![
        "help", "clear", "status", "stats", "devices", "report",
        "diag on", "diag off", "diag report",
        "test off", "test sine", "test chirp", "test noise",
        "loopback on", "loopback off",
        "set input", "set output"
    ];
    
    for cmd in commands {
        if cmd.starts_with(partial) && cmd != partial {
            return Some(cmd.to_string());
        }
    }
    None
}
