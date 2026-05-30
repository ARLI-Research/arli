//! Terminal UI for ARLI — ratatui-based interactive chat.
//!
//! Layout:
//! +---------------------------------+
//! |  ARLI v0.1  | session:id  |  <- header
//! +---------------------------------+
//! |                                 |
//! |  Message history (scrollable)   |  <- body
//! |                                 |
//! +---------------------------------+
//! | > user input here...            |  <- input
//! +---------------------------------+

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use arli_core::AgentMessage;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;
use tokio::sync::mpsc;

/// A single message in the chat history.
#[derive(Debug, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

/// TUI application state.
pub struct TuiApp {
    /// Agent message sender
    agent_tx: mpsc::Sender<AgentMessage>,
    /// Chat history
    messages: Vec<ChatMessage>,
    /// Current input buffer
    input: String,
    /// Cursor position in input
    cursor_pos: usize,
    /// Scroll position in message list
    scroll: usize,
    /// Is agent currently processing?
    running: bool,
    /// Status message
    status: String,
    /// Should we exit?
    should_quit: bool,
}

impl TuiApp {
    pub fn new(agent_tx: mpsc::Sender<AgentMessage>) -> Self {
        Self {
            agent_tx,
            messages: vec![ChatMessage {
                role: "system".into(),
                content: "ARLI Agent — type your message or /help".into(),
            }],
            input: String::new(),
            cursor_pos: 0,
            scroll: 0,
            running: false,
            status: "Ready".into(),
            should_quit: false,
        }
    }

    fn add_message(&mut self, role: &str, content: String) {
        self.messages.push(ChatMessage {
            role: role.to_string(),
            content,
        });
        self.scroll = self.messages.len().saturating_sub(1);
    }

    fn handle_input(&mut self) {
        let input = self.input.trim().to_string();
        if input.is_empty() {
            return;
        }

        match input.as_str() {
            "/quit" | "/exit" | "/q" => {
                self.should_quit = true;
                let _ = self.agent_tx.try_send(AgentMessage::Stop);
                return;
            }
            "/help" => {
                self.add_message("system",
                    "/help — Help\n/quit — Exit\n/clear — Clear chat".into()
                );
                self.input.clear();
                self.cursor_pos = 0;
                return;
            }
            "/clear" => {
                self.messages.clear();
                self.input.clear();
                self.cursor_pos = 0;
                return;
            }
            _ => {}
        }

        // Send to agent
        self.add_message("user", input.clone());
        self.running = true;
        self.status = "Processing...".into();

        let tx = self.agent_tx.clone();
        let _ = tx.try_send(AgentMessage::UserMessage(input));

        self.input.clear();
        self.cursor_pos = 0;
    }
}

/// Run the TUI — returns when user quits.
pub async fn run_tui(
    agent_tx: mpsc::Sender<AgentMessage>,
    mut agent_rx: mpsc::Receiver<String>,
) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(agent_tx);

    let result = run_app(&mut terminal, &mut app, &mut agent_rx).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp,
    agent_rx: &mut mpsc::Receiver<String>,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| draw_ui(f, app))?;

        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            msg = agent_rx.recv() => {
                match msg {
                    Some(response) => {
                        app.add_message("assistant", response);
                        app.running = false;
                        app.status = "Ready".into();
                    }
                    None => {
                        app.status = "Agent disconnected".into();
                    }
                }
            }

            _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => {
                if event::poll(std::time::Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        if key.kind == KeyEventKind::Press {
                            match key.code {
                                KeyCode::Enter => app.handle_input(),
                                KeyCode::Char(c) => {
                                    app.input.push(c);
                                    app.cursor_pos = app.input.len();
                                }
                                KeyCode::Backspace => {
                                    if app.cursor_pos > 0 {
                                        app.input.remove(app.cursor_pos - 1);
                                        app.cursor_pos -= 1;
                                    }
                                }
                                KeyCode::Left => {
                                    app.cursor_pos = app.cursor_pos.saturating_sub(1);
                                }
                                KeyCode::Right => {
                                    app.cursor_pos = (app.cursor_pos + 1).min(app.input.len());
                                }
                                KeyCode::Up => {
                                    app.scroll = app.scroll.saturating_sub(1);
                                }
                                KeyCode::Down => {
                                    app.scroll = (app.scroll + 1).min(
                                        app.messages.len().saturating_sub(1)
                                    );
                                }
                                KeyCode::PageUp => {
                                    app.scroll = app.scroll.saturating_sub(10);
                                }
                                KeyCode::PageDown => {
                                    app.scroll = (app.scroll + 10).min(
                                        app.messages.len().saturating_sub(1)
                                    );
                                }
                                KeyCode::Esc => {
                                    app.should_quit = true;
                                    let _ = app.agent_tx.try_send(AgentMessage::Stop);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn draw_ui(f: &mut Frame, app: &TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    draw_header(f, chunks[0], app);
    draw_body(f, chunks[1], app);
    draw_input(f, chunks[2], app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &TuiApp) {
    let header_text = vec![Line::from(vec![
        Span::styled(
            "  ARLI v0.1  ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("| {} | {}", app.status, if app.running { "*" } else { "-" }),
            Style::default().fg(Color::Gray),
        ),
    ])];

    let header = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::DarkGray)))
        .style(Style::default());
    f.render_widget(header, area);
}

fn draw_body(f: &mut Frame, area: Rect, app: &TuiApp) {
    let mut lines: Vec<Line> = Vec::new();

    // Build wrapped text from all messages
    for msg in &app.messages {
        let role_color = match msg.role.as_str() {
            "user" => Color::Cyan,
            "assistant" => Color::Green,
            "system" => Color::Yellow,
            _ => Color::Gray,
        };

        let prefix = match msg.role.as_str() {
            "user" => "> ",
            "assistant" => "",
            _ => "",
        };

        // Split content by newlines, then wrap each line
        for line in msg.content.lines() {
            let full_line = format!("{}{}", prefix, line);
            // Word-wrap for the body area width
            let max_width = area.width.saturating_sub(4) as usize;
            if full_line.len() > max_width && max_width > 20 {
                for chunk in full_line.as_bytes().chunks(max_width) {
                    let s = String::from_utf8_lossy(chunk);
                    lines.push(Line::from(Span::styled(
                        s.to_string(),
                        Style::default().fg(role_color),
                    )));
                }
            } else {
                lines.push(Line::from(Span::styled(
                    full_line,
                    Style::default().fg(role_color),
                )));
            }
        }
    }

    // Scroll: show last N lines that fit
    let visible_lines = area.height.saturating_sub(2) as usize;
    let start = lines.len().saturating_sub(visible_lines + app.scroll);
    let shown: Vec<Line> = lines.into_iter().skip(start).take(visible_lines).collect();

    let paragraph = Paragraph::new(shown)
        .block(Block::default().borders(Borders::ALL).style(Style::default()))
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn draw_input(f: &mut Frame, area: Rect, app: &TuiApp) {
    let input_text = format!("> {}", app.input);

    let cursor_style = if app.running {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White).add_modifier(Modifier::REVERSED)
    };

    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title(" Input (Esc to quit) "))
        .style(cursor_style);

    f.render_widget(input, area);

    if !app.running {
        f.set_cursor_position(ratatui::layout::Position::new(
            area.x + 2 + app.cursor_pos as u16,
            area.y + 1,
        ));
    }
}
