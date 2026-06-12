use crate::net::{Command, Event};
use crate::proto::parse_code;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const PAGE: usize = 5;

/// Which screen is showing. Carries that screen's state.
pub enum Screen {
    Discovering,
    Exchange {
        my_code: std::net::SocketAddr,
        input: String,
        error: Option<String>,
    },
    Punching {
        peer: std::net::SocketAddr,
    },
    Chat {
        peer: std::net::SocketAddr,
        messages: Vec<String>,
        input: String,
        scroll: usize, // lines from the bottom; 0 = pinned
        connected: bool,
    },
    Fatal {
        msg: String,
    },
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        App {
            screen: Screen::Discovering,
            should_quit: false,
        }
    }

    /// Apply an event from the network worker.
    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::Discovered(code) => {
                if matches!(self.screen, Screen::Discovering) {
                    self.screen = Screen::Exchange {
                        my_code: code,
                        input: String::new(),
                        error: None,
                    };
                }
            }
            Event::Connected(peer) => {
                self.screen = Screen::Chat {
                    peer,
                    messages: Vec::new(),
                    input: String::new(),
                    scroll: 0,
                    connected: true,
                };
            }
            Event::Incoming(s) => {
                if let Screen::Chat { messages, scroll, .. } = &mut self.screen {
                    messages.push(format!("peer> {s}"));
                    if *scroll > 0 {
                        let max = messages.len().saturating_sub(1);
                        *scroll = (*scroll + 1).min(max);
                    }
                }
            }
            Event::PeerLeft => {
                if let Screen::Chat {
                    messages, connected, ..
                } = &mut self.screen
                {
                    *connected = false;
                    messages.push("* peer disconnected *".to_string());
                }
            }
            Event::Fatal(msg) => {
                self.screen = Screen::Fatal { msg };
            }
        }
    }

    /// The worker's event channel disconnected unexpectedly.
    pub fn connection_lost(&mut self) {
        if !matches!(self.screen, Screen::Fatal { .. }) {
            self.screen = Screen::Fatal {
                msg: "connection lost — the network thread stopped".to_string(),
            };
        }
    }

    /// Handle a keypress. May mutate screen state and/or return a command to
    /// send to the worker.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        // Global quit: Esc or Ctrl-C from any screen.
        let ctrl_c = key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || ctrl_c {
            self.should_quit = true;
            return Some(Command::Quit);
        }

        let mut transition: Option<Screen> = None;
        let mut cmd: Option<Command> = None;
        let mut quit = false;

        match &mut self.screen {
            Screen::Exchange { input, error, .. } => match key.code {
                KeyCode::Char(c) => {
                    input.push(c);
                    *error = None;
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => match parse_code(input) {
                    Ok(addr) => {
                        transition = Some(Screen::Punching { peer: addr });
                        cmd = Some(Command::PeerCode(addr));
                    }
                    Err(e) => *error = Some(e.to_string()),
                },
                _ => {}
            },
            Screen::Chat {
                messages,
                input,
                scroll,
                ..
            } => match key.code {
                KeyCode::Char(c) => input.push(c),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    if !input.is_empty() {
                        let line = std::mem::take(input);
                        messages.push(format!("you> {line}"));
                        *scroll = 0;
                        cmd = Some(Command::Send(line));
                    }
                }
                KeyCode::PageUp => {
                    let max = messages.len().saturating_sub(1);
                    *scroll = (*scroll + PAGE).min(max);
                }
                KeyCode::PageDown => {
                    *scroll = scroll.saturating_sub(PAGE);
                }
                _ => {}
            },
            Screen::Fatal { .. } => {
                if key.code == KeyCode::Char('q') {
                    quit = true;
                }
            }
            _ => {} // Discovering, Punching: nothing but the global quit
        }

        if let Some(s) = transition {
            self.screen = s;
        }
        if quit {
            self.should_quit = true;
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn addr() -> std::net::SocketAddr {
        "203.0.113.5:54213".parse().unwrap()
    }

    fn chat_app() -> App {
        App {
            screen: Screen::Chat {
                peer: addr(),
                messages: Vec::new(),
                input: String::new(),
                scroll: 0,
                connected: true,
            },
            should_quit: false,
        }
    }

    #[test]
    fn discovered_moves_to_exchange() {
        let mut app = App::new();
        app.apply(Event::Discovered(addr()));
        assert!(matches!(app.screen, Screen::Exchange { .. }));
    }

    #[test]
    fn incoming_appends_message_in_chat() {
        let mut app = chat_app();
        app.apply(Event::Incoming("yo".into()));
        if let Screen::Chat { messages, .. } = &app.screen {
            assert_eq!(messages, &vec!["peer> yo".to_string()]);
        } else {
            panic!("expected Chat");
        }
    }

    #[test]
    fn submit_chat_line_emits_send_and_echoes() {
        let mut app = chat_app();
        if let Screen::Chat { input, .. } = &mut app.screen {
            *input = "hello".to_string();
        }
        let cmd = app.on_key(key(KeyCode::Enter));
        assert!(matches!(cmd, Some(Command::Send(ref s)) if s == "hello"));
        if let Screen::Chat { messages, input, .. } = &app.screen {
            assert_eq!(messages, &vec!["you> hello".to_string()]);
            assert!(input.is_empty());
        } else {
            panic!("expected Chat");
        }
    }

    #[test]
    fn scroll_clamps_at_both_bounds() {
        let mut app = chat_app();
        if let Screen::Chat { messages, .. } = &mut app.screen {
            *messages = vec!["a".into(), "b".into(), "c".into()]; // max scroll = 2
        }
        app.on_key(key(KeyCode::PageUp)); // +5, clamped to 2
        if let Screen::Chat { scroll, .. } = &app.screen {
            assert_eq!(*scroll, 2);
        }
        app.on_key(key(KeyCode::PageDown)); // -5, clamped to 0
        if let Screen::Chat { scroll, .. } = &app.screen {
            assert_eq!(*scroll, 0);
        }
    }

    #[test]
    fn bad_peer_code_shows_inline_error() {
        let mut app = App::new();
        app.apply(Event::Discovered(addr()));
        if let Screen::Exchange { input, .. } = &mut app.screen {
            *input = "garbage".to_string();
        }
        let cmd = app.on_key(key(KeyCode::Enter));
        assert!(cmd.is_none());
        match &app.screen {
            Screen::Exchange { error, .. } => assert!(error.is_some()),
            _ => panic!("should stay on Exchange"),
        }
    }

    #[test]
    fn esc_sets_quit_and_returns_quit_command() {
        let mut app = chat_app();
        let cmd = app.on_key(key(KeyCode::Esc));
        assert!(app.should_quit);
        assert!(matches!(cmd, Some(Command::Quit)));
    }

    #[test]
    fn fatal_event_sets_fatal_screen() {
        let mut app = App::new();
        app.apply(Event::Fatal("boom".into()));
        assert!(matches!(app.screen, Screen::Fatal { .. }));
    }
}
