use crate::app::{App, Screen};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Render the whole UI for the current screen.
pub fn draw(f: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Discovering => {
            centered(f, "Discovering your public address via STUN…", Color::Gray);
        }
        Screen::Exchange {
            my_code,
            input,
            error,
        } => draw_exchange(f, &my_code.to_string(), input, error.as_deref()),
        Screen::Punching { peer } => {
            centered(
                f,
                &format!("Punching through to {peer}…  (up to 60s)"),
                Color::Yellow,
            );
        }
        Screen::Chat {
            peer,
            messages,
            input,
            scroll,
            connected,
            ..
        } => draw_chat(f, &peer.to_string(), messages, input, *scroll, *connected),
        Screen::Fatal { msg } => {
            centered(f, &format!("{msg}\n\n(press q or Esc to quit)"), Color::Red)
        }
    }
}

fn centered(f: &mut Frame, text: &str, color: Color) {
    let p = Paragraph::new(text)
        .style(Style::default().fg(color))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" ramsit "));
    f.render_widget(p, f.area());
}

fn draw_exchange(f: &mut Frame, my_code: &str, input: &str, error: Option<&str>) {
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("Your code: "),
            Span::styled(my_code, Style::default().fg(Color::Green).bold()),
        ]),
        Line::from("Share it with your bro, then paste theirs below."),
        Line::from(""),
        Line::from(vec![
            Span::raw("Peer code: "),
            Span::raw(input),
            Span::raw("_"),
        ]),
        Line::from(""),
        Line::from(match error {
            Some(e) => Span::styled(e, Style::default().fg(Color::Red)),
            None => Span::raw("Press Enter to connect · Esc to quit"),
        }),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" ramsit "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, f.area());
}

fn draw_chat(
    f: &mut Frame,
    peer: &str,
    messages: &[String],
    input: &str,
    scroll: usize,
    connected: bool,
) {
    let areas = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area());
    let (history_area, input_area) = (areas[0], areas[1]);

    // Status bar = title of the history block.
    let (label, color) = if connected {
        ("connected", Color::Green)
    } else {
        ("disconnected", Color::Red)
    };
    let title = Line::from(vec![
        Span::raw(format!(" ramsit — peer {peer} ")),
        Span::styled("●", Style::default().fg(color)),
        Span::raw(format!(" {label} ")),
    ]);

    let inner_h = history_area.height.saturating_sub(2) as usize; // minus borders
    let lines = visible_lines(messages, inner_h, scroll);
    let history = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(history, history_area);

    let prompt = format!("> {input}");
    let input_p = Paragraph::new(prompt.as_str())
        .block(Block::default().borders(Borders::ALL).title(" message "));
    f.render_widget(input_p, input_area);
    place_cursor(f, input_area, input.chars().count());
}

/// Pick the slice of messages to show: bottom-anchored, offset up by `scroll`.
fn visible_lines(messages: &[String], height: usize, scroll: usize) -> Vec<Line<'_>> {
    if height == 0 || messages.is_empty() {
        return Vec::new();
    }
    let end = messages.len().saturating_sub(scroll);
    let start = end.saturating_sub(height);
    messages[start..end]
        .iter()
        .map(|m| Line::from(m.as_str()))
        .collect()
}

fn place_cursor(f: &mut Frame, area: Rect, input_chars: usize) {
    // "> " prefix (2) + chars, inside the left border (+1).
    let x = area.x + 1 + 2 + input_chars as u16;
    let y = area.y + 1;
    f.set_cursor_position((x.min(area.x + area.width.saturating_sub(2)), y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn discovering_screen_shows_prompt() {
        let s = render(&App::new(), 60, 10);
        assert!(s.contains("Discovering"));
    }

    #[test]
    fn chat_screen_shows_messages_and_status() {
        use crate::app::Screen;
        let app = App {
            screen: Screen::Chat {
                peer: "203.0.113.5:54213".parse().unwrap(),
                messages: vec!["peer> yo".into(), "you> hey".into()],
                input: "typing".into(),
                scroll: 0,
                connected: true,
                muted: false,
                input_vol: 100,
                output_vol: 100,
                voice: false,
            },
            should_quit: false,
        };
        let s = render(&app, 60, 10);
        assert!(s.contains("peer> yo"));
        assert!(s.contains("you> hey"));
        assert!(s.contains("connected"));
        assert!(s.contains("> typing"));
    }
}
