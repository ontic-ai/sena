use std::{
    io,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use ui_core::{CliSnapshot, ConversationLine, SignalKind, SignalLine, SpeakerRole};

#[cfg(not(windows))]
use opentui_rust::{BoxStyle, Renderer, Rgba, Style, enable_raw_mode, terminal_size};

#[cfg(windows)]
use std::io::Write;

#[cfg(windows)]
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    style::{Attribute, Color, Stylize, style},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
    SynchronizedUpdate,
};

const CONTENT_WIDTH: usize = 92;
const FRAME_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct CliView {
    snapshot: CliSnapshot,
}

enum CliCommand {
    Snapshot(CliSnapshot),
    Finish,
}

pub struct CliLiveSession {
    tx: Sender<CliCommand>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl CliLiveSession {
    pub fn update(&self, snapshot: CliSnapshot) -> io::Result<()> {
        self.tx
            .send(CliCommand::Snapshot(snapshot))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "CLI session closed"))
    }

    pub fn finish(mut self) -> io::Result<()> {
        let _ = self.tx.send(CliCommand::Finish);
        match self.join.take().expect("live session join handle must exist").join() {
            Ok(result) => result,
            Err(_) => Err(io::Error::other("CLI thread panicked")),
        }
    }
}

impl CliView {
    pub fn new(snapshot: CliSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn start_live(snapshot: CliSnapshot) -> CliLiveSession {
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut view = CliView::new(snapshot);

            #[cfg(not(windows))]
            {
                return view.stream_opentui(rx);
            }

            #[cfg(windows)]
            {
                return view.stream_windows_tui(rx);
            }

            #[allow(unreachable_code)]
            Err(io::Error::other("CLI built without a terminal backend"))
        });

        CliLiveSession {
            tx,
            join: Some(join),
        }
    }

    pub fn update(&mut self, snapshot: CliSnapshot) {
        self.snapshot = snapshot;
    }

    pub fn render_text_frame(&self) -> String {
        let mut lines = vec!["Conversation".to_owned()];
        lines.extend(self.snapshot.conversation.iter().map(render_conversation));
        lines.push(String::new());
        lines.push("Signals".to_owned());
        lines.extend(self.snapshot.signals.iter().map(render_signal));
        lines.join("\n")
    }

    fn apply_live_command(&mut self, command: CliCommand) -> bool {
        match command {
            CliCommand::Snapshot(snapshot) => {
                self.update(snapshot);
                true
            }
            CliCommand::Finish => false,
        }
    }

    fn drain_live_commands(&mut self, rx: &Receiver<CliCommand>) -> bool {
        loop {
            match rx.try_recv() {
                Ok(command) => {
                    if !self.apply_live_command(command) {
                        return false;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => return false,
            }
        }
    }
}

#[cfg(not(windows))]
impl CliView {
    fn stream_opentui(&mut self, rx: Receiver<CliCommand>) -> io::Result<()> {
        let _raw_mode = enable_raw_mode()?;
        let (width, height) = terminal_size()?;
        let mut renderer = Renderer::new(u32::from(width), u32::from(height))?;
        renderer.set_title("Sena CLI")?;
        renderer.set_cursor(0, 0, false)?;

        loop {
            let (width, height) = terminal_size()?;
            renderer.resize(u32::from(width), u32::from(height))?;
            self.draw(&mut renderer)?;

            match rx.recv_timeout(FRAME_INTERVAL) {
                Ok(command) => {
                    if !self.apply_live_command(command) {
                        break;
                    }
                    if !self.drain_live_commands(&rx) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        renderer.cleanup()?;
        Ok(())
    }

    fn draw(&self, renderer: &mut Renderer) -> io::Result<()> {
        let (width, height) = renderer.size();
        let frame = renderer.buffer();
        frame.clear(Rgba::from_rgb_u8(7, 13, 18));

        let layout = build_layout(&self.snapshot);
        let content_width = width.saturating_sub(6).max(1).min(layout.content_width as u32);
        let total_height = layout.lines.len() as u32;
        let card_width = content_width.saturating_add(4).min(width);
        let card_height = total_height.saturating_add(2).min(height);
        let card_x = width.saturating_sub(card_width) / 2;
        let card_y = height.saturating_sub(card_height) / 2;
        let left_x = if card_width > content_width + 1 {
            card_x.saturating_add(2)
        } else {
            width.saturating_sub(content_width) / 2
        };

        if card_width > 8 && card_height > 4 {
            frame.draw_box(
                card_x,
                card_y,
                card_width,
                card_height,
                BoxStyle::rounded(Style::fg(Rgba::from_rgb_u8(28, 181, 150))),
            );
        }

        let mut y = card_y.saturating_add(1);
        for line in &layout.lines {
            if y >= height.saturating_sub(1) {
                break;
            }

            let text = fit_text(&line.text, content_width as usize);
            let x = match line.align {
                LineAlign::Center => left_x.saturating_add(
                    content_width.saturating_sub(text.len() as u32) / 2,
                ),
                LineAlign::Left => left_x,
            };
            frame.draw_text(x, y, &text, tone_style(line.tone));
            y += 1;
        }

        renderer.present()
    }
}

#[cfg(windows)]
impl CliView {
    fn stream_windows_tui(&mut self, rx: Receiver<CliCommand>) -> io::Result<()> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, Hide)?;

        let result = self.stream_windows_frames(&mut stdout, rx);

        execute!(stdout, Show, LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        result
    }

    fn stream_windows_frames(
        &mut self,
        stdout: &mut io::Stdout,
        rx: Receiver<CliCommand>,
    ) -> io::Result<()> {
        loop {
            self.draw_windows(stdout)?;

            match rx.recv_timeout(FRAME_INTERVAL) {
                Ok(command) => {
                    if !self.apply_live_command(command) {
                        break;
                    }
                    if !self.drain_live_commands(&rx) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    fn draw_windows(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        let (width, height) = terminal::size()?;
        let layout = build_layout(&self.snapshot);
        let content_width = width.saturating_sub(6).max(1).min(layout.content_width as u16) as usize;
        let top_padding = center_offset(height as usize, layout.lines.len().saturating_add(2));
        let block_padding = center_offset(width as usize, content_width.saturating_add(4));
        let left_padding = block_padding.saturating_add(2);
        let divider = format!(
            "{}{}",
            " ".repeat(block_padding),
            fit_text(&"-".repeat(content_width), content_width)
        );

        stdout.sync_update(|out| {
            execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;

            for _ in 0..top_padding {
                writeln!(out)?;
            }

            writeln!(out, "{}", paint_windows(divider.clone(), Tone::Divider))?;

            for line in &layout.lines {
                let text = fit_text(&line.text, content_width);
                let padding = match line.align {
                    LineAlign::Center => center_offset(width as usize, text.len()),
                    LineAlign::Left => left_padding,
                };
                let content = format!("{}{}", " ".repeat(padding), text);
                writeln!(out, "{}", paint_windows(content, line.tone))?;
            }

            writeln!(out, "{}", paint_windows(divider, Tone::Divider))?;
            out.flush()
        })?
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Brand,
    Divider,
    Muted,
    Accent,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineAlign {
    Left,
    Center,
}

#[derive(Debug, Clone)]
struct LayoutLine {
    text: String,
    tone: Tone,
    align: LineAlign,
}

#[derive(Debug, Clone)]
struct CliLayout {
    content_width: usize,
    lines: Vec<LayoutLine>,
}

fn build_layout(snapshot: &CliSnapshot) -> CliLayout {
    let mut lines = vec![
        centered_line("SENA-CLI".to_owned(), Tone::Brand),
        centered_line("developer subscriber surface".to_owned(), Tone::Muted),
        blank_line(),
        centered_line("-".repeat(40), Tone::Divider),
        blank_line(),
        left_line("Conversation".to_owned(), Tone::Accent),
    ];

    if snapshot.conversation.is_empty() {
        lines.push(left_line("  waiting for conversation events".to_owned(), Tone::Muted));
    } else {
        lines.extend(snapshot.conversation.iter().map(render_conversation_line));
    }

    lines.push(blank_line());
    lines.push(left_line("Signals".to_owned(), Tone::Accent));

    if snapshot.signals.is_empty() {
        lines.push(left_line("  waiting for runtime signals".to_owned(), Tone::Muted));
    } else {
        lines.extend(snapshot.signals.iter().map(render_signal_line));
    }

    lines.push(blank_line());
    lines.push(centered_line(
        "Ctrl+C stops this development subscriber surface".to_owned(),
        Tone::Muted,
    ));

    CliLayout {
        content_width: CONTENT_WIDTH,
        lines,
    }
}

fn render_conversation(line: &ConversationLine) -> String {
    let role = match line.role {
        SpeakerRole::User => "USER",
        SpeakerRole::Assistant => "SENA",
        SpeakerRole::System => "SYS",
    };
    format!("[{}] {}", role, line.text)
}

fn render_conversation_line(line: &ConversationLine) -> LayoutLine {
    let tone = match line.role {
        SpeakerRole::Assistant => Tone::Complete,
        SpeakerRole::User => Tone::Accent,
        SpeakerRole::System => Tone::Muted,
    };
    left_line(format!("  {}", render_conversation(line)), tone)
}

fn render_signal(line: &SignalLine) -> String {
    let label = match line.kind {
        SignalKind::SttPartial => "STT~",
        SignalKind::SttFinal => "STT",
        SignalKind::Think => "THINK",
        SignalKind::Task => "TASK",
        SignalKind::Cancel => "CANCEL",
        SignalKind::Sri => "SRI",
        SignalKind::Download => "DL",
        SignalKind::Fault => "FAULT",
        SignalKind::Info => "INFO",
    };
    format!("[{}] {}", label, line.text)
}

fn render_signal_line(line: &SignalLine) -> LayoutLine {
    let tone = match line.kind {
        SignalKind::Fault => Tone::Failed,
        SignalKind::Download => Tone::Accent,
        SignalKind::Sri => Tone::Complete,
        SignalKind::Info => Tone::Muted,
        _ => Tone::Accent,
    };
    left_line(format!("  {}", render_signal(line)), tone)
}

fn centered_line(text: String, tone: Tone) -> LayoutLine {
    LayoutLine {
        text,
        tone,
        align: LineAlign::Center,
    }
}

fn left_line(text: String, tone: Tone) -> LayoutLine {
    LayoutLine {
        text,
        tone,
        align: LineAlign::Left,
    }
}

fn blank_line() -> LayoutLine {
    LayoutLine {
        text: String::new(),
        tone: Tone::Muted,
        align: LineAlign::Left,
    }
}

fn fit_text(text: &str, max_width: usize) -> String {
    if text.len() <= max_width {
        return text.to_owned();
    }

    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    format!("{}...", &text[..max_width - 3])
}

fn center_offset(total: usize, used: usize) -> usize {
    total.saturating_sub(used) / 2
}

#[cfg(not(windows))]
fn tone_style(tone: Tone) -> Style {
    match tone {
        Tone::Brand => Style::fg(Rgba::from_rgb_u8(33, 220, 176)).with_bold(),
        Tone::Divider => Style::fg(Rgba::from_rgb_u8(28, 181, 150)),
        Tone::Muted => Style::fg(Rgba::from_rgb_u8(126, 145, 158)),
        Tone::Accent => Style::fg(Rgba::from_rgb_u8(114, 224, 196)).with_bold(),
        Tone::Complete => Style::fg(Rgba::from_rgb_u8(86, 214, 124)).with_bold(),
        Tone::Failed => Style::fg(Rgba::from_rgb_u8(255, 111, 97)).with_bold(),
    }
}

#[cfg(windows)]
fn paint_windows(text: String, tone: Tone) -> crossterm::style::StyledContent<String> {
    let background = Color::Rgb { r: 7, g: 13, b: 18 };
    match tone {
        Tone::Brand => style(text)
            .with(Color::Rgb {
                r: 33,
                g: 220,
                b: 176,
            })
            .on(background)
            .attribute(Attribute::Bold),
        Tone::Divider => style(text)
            .with(Color::Rgb {
                r: 28,
                g: 181,
                b: 150,
            })
            .on(background),
        Tone::Muted => style(text)
            .with(Color::Rgb {
                r: 126,
                g: 145,
                b: 158,
            })
            .on(background),
        Tone::Accent => style(text)
            .with(Color::Rgb {
                r: 114,
                g: 224,
                b: 196,
            })
            .on(background)
            .attribute(Attribute::Bold),
        Tone::Complete => style(text)
            .with(Color::Rgb {
                r: 86,
                g: 214,
                b: 124,
            })
            .on(background)
            .attribute(Attribute::Bold),
        Tone::Failed => style(text)
            .with(Color::Rgb {
                r: 255,
                g: 111,
                b: 97,
            })
            .on(background)
            .attribute(Attribute::Bold),
    }
}
