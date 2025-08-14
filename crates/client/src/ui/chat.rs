use fltk::{prelude::*, *};
use std::sync::{Arc, Mutex};

use crate::network::NetworkClient;
use crate::settings::Settings;
use crate::ui::theme::DarkTheme;

pub struct Chat {
    network_client: Arc<Mutex<Option<NetworkClient>>>,
    settings: Arc<Mutex<Settings>>,

    // UI elements
    chat_display: Option<text::TextDisplay>,
    chat_buffer: Option<text::TextBuffer>,
    style_buffer: Option<text::TextBuffer>,
    message_input: Option<input::Input>,
}

impl Chat {
    pub fn new(
        network_client: Arc<Mutex<Option<NetworkClient>>>,
        settings: Arc<Mutex<Settings>>,
    ) -> Self {
        Chat {
            network_client,
            settings,
            chat_display: None,
            chat_buffer: None,
            style_buffer: None,
            message_input: None,
        }
    }

    pub fn create_ui(&mut self) -> group::Flex {
        let mut chat_col = group::Flex::default().column();
        chat_col.set_margins(10, 10, 10, 10);
        chat_col.set_spacing(10);

        // Chat display with styled text
        let mut chat_display = text::TextDisplay::default();
        let mut chat_buffer = text::TextBuffer::default();
        let mut style_buffer = text::TextBuffer::default();

        chat_display.set_buffer(chat_buffer.clone());
        chat_display.wrap_mode(text::WrapMode::AtBounds, 0);

        // Custom styling for the display background
        chat_display.set_color(DarkTheme::BG_INPUT);
        chat_display.set_cursor_color(DarkTheme::ACCENT);
        chat_display.set_selection_color(DarkTheme::ACCENT);
        chat_display.set_frame(enums::FrameType::BorderBox);

        // Create style table for different text styles
        let style_table = vec![
            text::StyleTableEntryExt {
                color: DarkTheme::TEXT_MUTED, // A: Timestamp (gray)
                font: enums::Font::Helvetica,
                size: 12,
                attr: text::TextAttr::None,
                bgcolor: DarkTheme::BG_INPUT,
            },
            text::StyleTableEntryExt {
                color: DarkTheme::ACCENT, // B: Other usernames (blue)
                font: enums::Font::HelveticaBold,
                size: 13,
                attr: text::TextAttr::None,
                bgcolor: DarkTheme::BG_INPUT,
            },
            text::StyleTableEntryExt {
                color: DarkTheme::TEXT_SECONDARY, // C: Message text (light gray)
                font: enums::Font::Helvetica,
                size: 13,
                attr: text::TextAttr::None,
                bgcolor: DarkTheme::BG_INPUT,
            },
            text::StyleTableEntryExt {
                color: DarkTheme::TEXT_MUTED, // D: System messages (muted)
                font: enums::Font::HelveticaItalic,
                size: 12,
                attr: text::TextAttr::None,
                bgcolor: DarkTheme::BG_INPUT,
            },
            text::StyleTableEntryExt {
                color: DarkTheme::SUCCESS, // E: Your username (green)
                font: enums::Font::HelveticaBold,
                size: 13,
                attr: text::TextAttr::None,
                bgcolor: DarkTheme::BG_INPUT,
            },
            text::StyleTableEntryExt {
                color: DarkTheme::ACCENT, // F: Links (blue with underline)
                font: enums::Font::Helvetica,
                size: 13,
                attr: text::TextAttr::Underline,
                bgcolor: DarkTheme::BG_INPUT,
            },
        ];

        chat_display.set_highlight_data_ext(style_buffer.clone(), style_table);

        // Initial message
        self.append_system_message("Connecting to server...\n");

        self.chat_display = Some(chat_display.clone());
        self.chat_buffer = Some(chat_buffer);
        self.style_buffer = Some(style_buffer);

        // For now, we'll skip link clicking as it requires more complex handling
        // You could implement this by tracking click positions and matching against stored URL positions

        // Input row
        let mut input_row = group::Flex::default().row();
        chat_col.fixed(&input_row, 40);
        input_row.set_spacing(10);

        let mut message_input = input::Input::default();
        message_input.set_trigger(enums::CallbackTrigger::EnterKeyAlways);
        message_input.set_maximum_size(400); // 400 character limit
        DarkTheme::style_input(&mut message_input);
        self.message_input = Some(message_input.clone());

        // Character counter
        let mut char_count = frame::Frame::default().with_label("0/400");
        input_row.fixed(&char_count, 60);
        char_count.set_label_color(DarkTheme::TEXT_MUTED);
        char_count.set_label_size(11);

        let mut send_btn = button::Button::default().with_label("Send");
        input_row.fixed(&send_btn, 80);
        send_btn.set_color(DarkTheme::ACCENT);
        send_btn.set_label_color(enums::Color::White);
        send_btn.set_frame(enums::FrameType::FlatBox);
        let mut char_count_clone = char_count.clone();
        input_row.end();

        // Update character counter on input change
        let mut message_input_clone = message_input.clone();
        message_input_clone.handle(move |input, event| {
            match event {
                enums::Event::KeyUp | enums::Event::KeyDown => {
                    let len = input.value().len();
                    char_count_clone.set_label(&format!("{}/400", len));
                    if len > 380 {
                        char_count_clone.set_label_color(DarkTheme::WARNING);
                    } else {
                        char_count_clone.set_label_color(DarkTheme::TEXT_MUTED);
                    }
                    char_count_clone.redraw();
                    false // Don't consume the event
                }
                _ => false,
            }
        });

        self.setup_message_handlers(message_input.clone(), send_btn);

        chat_col.end();
        chat_col
    }

    fn setup_message_handlers(
        &self,
        mut message_input: input::Input,
        mut send_btn: button::Button,
    ) {
        let network_client = self.network_client.clone();
        let settings = self.settings.clone();
        let mut message_input_clone = message_input.clone();

        let mut send_message = move || {
            let content = message_input_clone.value();
            if !content.trim().is_empty() && content.len() <= 400 {
                // Local echo - show message immediately (commented out to avoid duplicates)
                // let nickname = settings.lock().unwrap().nickname.clone();

                // Send to network
                if let Ok(mut client_guard) = network_client.lock() {
                    if let Some(client) = client_guard.as_mut() {
                        client.send_message(content.clone());
                    }
                }

                message_input_clone.set_value("");
                message_input_clone.do_callback(); // Update char counter
            }
        };

        {
            let mut send_msg_clone = send_message.clone();
            message_input.handle(move |_input, event| {
                // Only send on Enter key
                if event == enums::Event::KeyDown && app::event_key() == enums::Key::Enter {
                    send_msg_clone();
                    true // Consume the event
                } else {
                    false // Let other handlers process it
                }
            });
        }

        send_btn.set_callback(move |_| send_message());
    }

    // Helper to append styled text
    fn append_styled(&mut self, text: &str, styles: &str) {
        if let (Some(chat_buffer), Some(style_buffer)) =
            (&mut self.chat_buffer, &mut self.style_buffer)
        {
            chat_buffer.append(text);
            style_buffer.append(styles);
        }
    }

    // Helper for system messages
    fn append_system_message(&mut self, text: &str) {
        let style = "D".repeat(text.len());
        self.append_styled(text, &style);
    }

    // Auto-scroll to bottom
    fn scroll_to_bottom(&mut self) {
        if let Some(display) = &mut self.chat_display {
            if let Some(buffer) = &self.chat_buffer {
                let length = buffer.length();
                display.set_insert_position(length);
                display.scroll(display.count_lines(0, length, true), 0);
            }
        }
    }

    pub fn handle_init(&mut self, _server_name: String, greeting: String) {
        // Clear initial message
        if let (Some(chat_buffer), Some(style_buffer)) =
            (&mut self.chat_buffer, &mut self.style_buffer)
        {
            chat_buffer.set_text("");
            style_buffer.set_text("");
        }

        // Just show the greeting, no decorations
        let greeting_msg = format!("{}\n\n", greeting);
        self.append_system_message(&greeting_msg);

        self.scroll_to_bottom();
        app::redraw(); // Force immediate redraw instead of awake
    }

    pub fn handle_connected(&mut self) {
        self.scroll_to_bottom();
        app::awake();
    }

    pub fn handle_disconnected(&mut self, reason: String) {
        let msg = format!("Disconnected: {}\n", reason);
        self.append_system_message(&msg);
        self.scroll_to_bottom();
        app::awake();
    }

    pub fn handle_user_joined(&mut self, username: String) {
        let msg = format!("{} joined\n", username);
        self.append_system_message(&msg);
        self.scroll_to_bottom();
        app::awake();
    }

    pub fn handle_user_left(&mut self, username: String) {
        let msg = format!("{} left\n", username);
        self.append_system_message(&msg);
        self.scroll_to_bottom();
        app::awake();
    }

    pub fn handle_text_message(&mut self, username: String, text: String) {
        // Check if this is our own message (skip local echo)
        let is_own_message = {
            let settings = self.settings.lock().unwrap();
            username == settings.nickname
        };

        // Format: [HH:MM] Username: Message
        let timestamp = format!("[{}] ", chrono::Local::now().format("%H:%M"));
        let username_part = format!("{}: ", username);

        // Process message for URLs
        let message_with_styles = self.process_message_with_links(&text);
        let message_part = format!("{}\n", text);

        // Append with appropriate styles
        self.append_styled(&timestamp, &"A".repeat(timestamp.len())); // Gray timestamp

        // Username color based on if it's our message
        let username_style = if is_own_message { 'E' } else { 'B' };
        self.append_styled(
            &username_part,
            &username_style.to_string().repeat(username_part.len()),
        );

        // Message with potential link styling
        self.append_styled(&message_part, &message_with_styles);

        self.scroll_to_bottom();
        app::awake();
    }

    // Process message text to detect and style URLs
    fn process_message_with_links(&self, text: &str) -> String {
        let mut styles = String::new();
        let mut in_url = false;

        for ch in text.chars() {
            // Simple URL detection - check for http:// or https://
            if !in_url && text.contains("http://") || text.contains("https://") {
                // This is simplified - you'd want better URL detection
                in_url = true;
            }

            if in_url && ch.is_whitespace() {
                in_url = false;
            }

            styles.push(if in_url { 'F' } else { 'C' });
        }

        styles.push('C'); // For the newline
        styles
    }

    pub fn clear(&mut self) {
        if let (Some(chat_buffer), Some(style_buffer)) =
            (&mut self.chat_buffer, &mut self.style_buffer)
        {
            chat_buffer.set_text("");
            style_buffer.set_text("");
        }
        if let Some(message_input) = &mut self.message_input {
            message_input.set_value("");
        }
    }
}
