use std::{
    io,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use ui_core::{BootstrapSnapshot, LoaderActorSnapshot, LoaderStatus};

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

const CONTENT_WIDTH: usize = 54;
const PROGRESS_BAR_WIDTH: usize = 18;
const FRAME_INTERVAL: Duration = Duration::from_millis(120);
const PREVIEW_HOLD: Duration = Duration::from_millis(950);
const SPINNER_FRAMES: [char; 4] = ['-', '\\', '|', '/'];

#[derive(Debug, Clone)]
pub struct BootstrapPlaybackStep {
    pub snapshot: BootstrapSnapshot,
    pub hold_for: Duration,
}

impl BootstrapPlaybackStep {
    pub fn new(snapshot: BootstrapSnapshot, hold_for: Duration) -> Self {
        Self { snapshot, hold_for }
    }
}

#[derive(Debug, Clone)]
pub struct BootstrapApp {
    snapshot: BootstrapSnapshot,
    expanded_actor_index: Option<usize>,
}

enum BootstrapCommand {
    Snapshot(BootstrapSnapshot),
    Finish,
}

pub struct BootstrapLiveSession {
    tx: Sender<BootstrapCommand>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl BootstrapLiveSession {
    pub fn update(&self, snapshot: BootstrapSnapshot) -> io::Result<()> {
        self.tx
            .send(BootstrapCommand::Snapshot(snapshot))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "bootstrapper session closed"))
    }

    pub fn finish(mut self) -> io::Result<()> {
        let _ = self.tx.send(BootstrapCommand::Finish);
        match self.join.take().expect("live session join handle must exist").join() {
            Ok(result) => result,
            Err(_) => Err(io::Error::other("bootstrapper thread panicked")),
        }
    }
}

impl BootstrapApp {
    pub fn new(snapshot: BootstrapSnapshot) -> Self {
        let expanded_actor_index = resolve_expanded_actor_index(&snapshot);
        Self {
            snapshot,
            expanded_actor_index,
        }
    }

    pub fn update(&mut self, snapshot: BootstrapSnapshot) {
        self.snapshot = snapshot;
        self.expanded_actor_index = resolve_expanded_actor_index(&self.snapshot);
    }

    pub fn run(&mut self) -> io::Result<()> {
        self.play([BootstrapPlaybackStep::new(
            self.snapshot.clone(),
            PREVIEW_HOLD,
        )])
    }

    pub fn start_live(snapshot: BootstrapSnapshot) -> BootstrapLiveSession {
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut app = BootstrapApp::new(snapshot);

            #[cfg(not(windows))]
            {
                return app.stream_opentui(rx);
            }

            #[cfg(windows)]
            {
                return app.stream_windows_tui(rx);
            }

            #[allow(unreachable_code)]
            Err(io::Error::other("bootstrap built without a loader backend"))
        });

        BootstrapLiveSession {
            tx,
            join: Some(join),
        }
    }

    pub fn play<I>(&mut self, steps: I) -> io::Result<()>
    where
        I: IntoIterator<Item = BootstrapPlaybackStep>,
    {
        let steps: Vec<_> = steps.into_iter().collect();
        if steps.is_empty() {
            return Ok(());
        }

        #[cfg(not(windows))]
        {
            return self.play_opentui(&steps);
        }

        #[cfg(windows)]
        {
            return self.play_windows_tui(&steps);
        }

        #[allow(unreachable_code)]
        Err(io::Error::other("bootstrap built without a loader backend"))
    }

    pub fn render_text_frame(&self) -> String {
        build_layout(&self.snapshot, self.expanded_actor_index, 0)
            .lines
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn uses_opentui_backend() -> bool {
    cfg!(not(windows))
}

#[cfg(not(windows))]
impl BootstrapApp {
    fn play_opentui(&mut self, steps: &[BootstrapPlaybackStep]) -> io::Result<()> {
        let _raw_mode = enable_raw_mode()?;
        let (width, height) = terminal_size()?;
        let mut renderer = Renderer::new(u32::from(width), u32::from(height))?;
        renderer.set_title(&self.snapshot.title)?;
        renderer.set_cursor(0, 0, false)?;

        let mut spinner_frame = 0;
        for step in steps {
            self.update(step.snapshot.clone());
            spinner_frame = advance_frames(spinner_frame, step.hold_for, |frame_index| {
                let (width, height) = terminal_size()?;
                renderer.resize(u32::from(width), u32::from(height))?;
                self.draw(&mut renderer, frame_index)
            })?;
        }

        renderer.cleanup()?;
        Ok(())
    }

    fn stream_opentui(&mut self, rx: Receiver<BootstrapCommand>) -> io::Result<()> {
        let _raw_mode = enable_raw_mode()?;
        let (width, height) = terminal_size()?;
        let mut renderer = Renderer::new(u32::from(width), u32::from(height))?;
        renderer.set_title(&self.snapshot.title)?;
        renderer.set_cursor(0, 0, false)?;

        let mut spinner_frame = 0;
        loop {
            let (width, height) = terminal_size()?;
            renderer.resize(u32::from(width), u32::from(height))?;
            self.draw(&mut renderer, spinner_frame)?;
            spinner_frame = (spinner_frame + 1) % SPINNER_FRAMES.len();

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

    fn draw(&self, renderer: &mut Renderer, spinner_frame: usize) -> io::Result<()> {
        let (width, height) = renderer.size();
        let frame = renderer.buffer();
        frame.clear(Rgba::from_rgb_u8(7, 13, 18));

        let layout = build_layout(&self.snapshot, self.expanded_actor_index, spinner_frame);
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
impl BootstrapApp {
    fn play_windows_tui(&mut self, steps: &[BootstrapPlaybackStep]) -> io::Result<()> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, Hide)?;

        let result = self.play_windows_frames(&mut stdout, steps);

        execute!(stdout, Show, LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        result
    }

    fn stream_windows_tui(&mut self, rx: Receiver<BootstrapCommand>) -> io::Result<()> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, Hide)?;

        let result = self.stream_windows_frames(&mut stdout, rx);

        execute!(stdout, Show, LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        result
    }

    fn play_windows_frames(
        &mut self,
        stdout: &mut io::Stdout,
        steps: &[BootstrapPlaybackStep],
    ) -> io::Result<()> {
        let mut spinner_frame = 0;
        for step in steps {
            self.update(step.snapshot.clone());
            spinner_frame = advance_frames(spinner_frame, step.hold_for, |frame_index| {
                self.draw_windows(stdout, frame_index)
            })?;
        }

        Ok(())
    }

    fn stream_windows_frames(
        &mut self,
        stdout: &mut io::Stdout,
        rx: Receiver<BootstrapCommand>,
    ) -> io::Result<()> {
        let mut spinner_frame = 0;
        loop {
            self.draw_windows(stdout, spinner_frame)?;
            spinner_frame = (spinner_frame + 1) % SPINNER_FRAMES.len();

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

    fn draw_windows(&self, stdout: &mut io::Stdout, spinner_frame: usize) -> io::Result<()> {
        let (width, height) = terminal::size()?;
        let layout = build_layout(&self.snapshot, self.expanded_actor_index, spinner_frame);
        let content_width = width.saturating_sub(6).max(1).min(layout.content_width as u16) as usize;
        let top_padding = center_offset_usize(height as usize, layout.lines.len().saturating_add(2));
        let block_padding = center_offset_usize(width as usize, content_width.saturating_add(4));
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
                    LineAlign::Center => center_offset_usize(width as usize, text.len()),
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
    Pending,
    Running,
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
struct BootstrapLayout {
    content_width: usize,
    lines: Vec<LayoutLine>,
}

impl BootstrapApp {
    fn apply_live_command(&mut self, command: BootstrapCommand) -> bool {
        match command {
            BootstrapCommand::Snapshot(snapshot) => {
                self.update(snapshot);
                true
            }
            BootstrapCommand::Finish => false,
        }
    }

    fn drain_live_commands(&mut self, rx: &Receiver<BootstrapCommand>) -> bool {
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

fn build_layout(
    snapshot: &BootstrapSnapshot,
    expanded_actor_index: Option<usize>,
    spinner_frame: usize,
) -> BootstrapLayout {
    let mut lines = vec![
        centered_line(brand_title(&snapshot.title), Tone::Brand),
        centered_line("bootstrapper".to_owned(), Tone::Muted),
        blank_line(),
        centered_line("-".repeat(34), Tone::Divider),
        blank_line(),
        centered_line(actor_chip_line(snapshot, spinner_frame), Tone::Muted),
        blank_line(),
    ];

    let (headline, headline_tone) = headline_line(snapshot);
    lines.push(centered_line(headline, headline_tone));
    lines.push(blank_line());

    if snapshot.actors.is_empty() {
        lines.push(left_line("    waiting for loader actors".to_owned(), Tone::Muted));
        lines.push(blank_line());
    } else {
        for (index, actor) in snapshot.actors.iter().enumerate() {
            let expanded = expanded_actor_index == Some(index);
            lines.push(left_line(
                actor_summary_line(actor, expanded, spinner_frame),
                tone_for_status(actor.status),
            ));

            if expanded {
                for subprocess in &actor.subprocesses {
                    lines.push(left_line(
                        format!(
                            "    {} {}",
                            status_icon(subprocess.status, spinner_frame),
                            fit_text(&subprocess.name, CONTENT_WIDTH.saturating_sub(7))
                        ),
                        tone_for_status(subprocess.status),
                    ));
                }

                if !actor.subprocesses.is_empty() {
                    lines.push(blank_line());
                }
            }
        }
    }

    lines.push(centered_line(footer_line(snapshot), Tone::Muted));

    BootstrapLayout {
        content_width: CONTENT_WIDTH,
        lines,
    }
}

fn brand_title(title: &str) -> String {
    title.trim().to_ascii_uppercase().replace(' ', "-")
}

fn actor_chip_line(snapshot: &BootstrapSnapshot, spinner_frame: usize) -> String {
    snapshot
        .actors
        .iter()
        .map(|actor| format!("{} {}", status_icon(actor.status, spinner_frame), actor.name))
        .collect::<Vec<_>>()
        .join("   ")
}

fn headline_line(snapshot: &BootstrapSnapshot) -> (String, Tone) {
    if snapshot
        .actors
        .iter()
        .any(|actor| actor.status == LoaderStatus::Failed)
    {
        return ("boot blocked by a failing actor".to_owned(), Tone::Failed);
    }

    if let Some(active_actor) = snapshot.active_actor.as_deref() {
        return (format!("loading {}...", active_actor), Tone::Running);
    }

    if snapshot
        .actors
        .iter()
        .all(|actor| actor.status == LoaderStatus::Complete)
    {
        return ("bootstrapper complete".to_owned(), Tone::Complete);
    }

    ("preparing loader scene".to_owned(), Tone::Muted)
}

fn footer_line(snapshot: &BootstrapSnapshot) -> String {
    if snapshot
        .actors
        .iter()
        .all(|actor| actor.status == LoaderStatus::Complete)
    {
        "runtime stays UI-agnostic; CLI remains a separate subscriber surface".to_owned()
    } else {
        "the active actor opens automatically while deep logs stay off-screen".to_owned()
    }
}

fn actor_summary_line(actor: &LoaderActorSnapshot, expanded: bool, spinner_frame: usize) -> String {
    let marker = if expanded { '>' } else { ' ' };
    let name = format!("{:<12}", fit_text(&actor.name, 12));
    format!(
        "{} {} {} {} {:>3}%",
        marker,
        status_icon(actor.status, spinner_frame),
        name,
        progress_bar(actor.progress_percent, PROGRESS_BAR_WIDTH),
        actor.progress_percent,
    )
}

fn progress_bar(progress_percent: u8, width: usize) -> String {
    let filled = ((usize::from(progress_percent) * width) + 99) / 100;
    format!(
        "[{}{}]",
        "#".repeat(filled.min(width)),
        ".".repeat(width.saturating_sub(filled.min(width)))
    )
}

fn status_icon(status: LoaderStatus, spinner_frame: usize) -> char {
    match status {
        LoaderStatus::Pending => '.',
        LoaderStatus::Running => SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()],
        LoaderStatus::Complete => '+',
        LoaderStatus::Failed => 'x',
    }
}

fn resolve_expanded_actor_index(snapshot: &BootstrapSnapshot) -> Option<usize> {
    snapshot
        .active_actor
        .as_deref()
        .and_then(|name| snapshot.actors.iter().position(|actor| actor.name == name))
        .or_else(|| {
            snapshot
                .actors
                .iter()
                .position(|actor| actor.status == LoaderStatus::Running)
        })
        .or_else(|| {
            snapshot
                .actors
                .iter()
                .position(|actor| actor.status == LoaderStatus::Pending)
        })
}

fn tone_for_status(status: LoaderStatus) -> Tone {
    match status {
        LoaderStatus::Pending => Tone::Pending,
        LoaderStatus::Running => Tone::Running,
        LoaderStatus::Complete => Tone::Complete,
        LoaderStatus::Failed => Tone::Failed,
    }
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

fn center_offset_usize(total: usize, used: usize) -> usize {
    total.saturating_sub(used) / 2
}

fn advance_frames<F>(
    mut spinner_frame: usize,
    hold_for: Duration,
    mut draw_frame: F,
) -> io::Result<usize>
where
    F: FnMut(usize) -> io::Result<()>,
{
    let started = Instant::now();

    loop {
        draw_frame(spinner_frame)?;
        spinner_frame = (spinner_frame + 1) % SPINNER_FRAMES.len();

        let elapsed = started.elapsed();
        if elapsed >= hold_for {
            return Ok(spinner_frame);
        }

        thread::sleep((hold_for - elapsed).min(FRAME_INTERVAL));
    }
}

#[cfg(not(windows))]
fn tone_style(tone: Tone) -> Style {
    match tone {
        Tone::Brand => Style::fg(Rgba::from_rgb_u8(33, 220, 176)).with_bold(),
        Tone::Divider => Style::fg(Rgba::from_rgb_u8(28, 181, 150)),
        Tone::Muted => Style::fg(Rgba::from_rgb_u8(126, 145, 158)),
        Tone::Pending => Style::fg(Rgba::from_rgb_u8(132, 150, 164)),
        Tone::Running => Style::fg(Rgba::from_rgb_u8(114, 224, 196)).with_bold(),
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
        Tone::Pending => style(text)
            .with(Color::Rgb {
                r: 132,
                g: 150,
                b: 164,
            })
            .on(background),
        Tone::Running => style(text)
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

#[cfg(test)]
mod tests {
    use super::BootstrapApp;
    use ui_core::{
        BootstrapSnapshot, LoaderActorSnapshot, LoaderStatus, LoaderSubprocessSnapshot,
    };

    #[test]
    fn expands_the_active_actor_automatically() {
        let app = BootstrapApp::new(snapshot(Some("runtime"), LoaderStatus::Running));

        assert_eq!(app.expanded_actor_index, Some(1));
        assert!(app.render_text_frame().contains("seed sri capability tree"));
        assert!(!app.render_text_frame().contains("load runtime config"));
    }

    #[test]
    fn collapses_actor_details_when_boot_is_complete() {
        let app = BootstrapApp::new(snapshot(None, LoaderStatus::Complete));

        assert_eq!(app.expanded_actor_index, None);
        assert!(app.render_text_frame().contains("bootstrapper complete"));
    }

    fn snapshot(active_actor: Option<&str>, runtime_status: LoaderStatus) -> BootstrapSnapshot {
        BootstrapSnapshot {
            title: "Sena Bootstrapper".to_owned(),
            active_actor: active_actor.map(str::to_owned),
            actors: vec![
                LoaderActorSnapshot {
                    name: "config".to_owned(),
                    status: LoaderStatus::Complete,
                    progress_percent: 100,
                    subprocesses: vec![LoaderSubprocessSnapshot {
                        name: "load runtime config".to_owned(),
                        status: LoaderStatus::Complete,
                    }],
                },
                LoaderActorSnapshot {
                    name: "runtime".to_owned(),
                    status: runtime_status,
                    progress_percent: if runtime_status == LoaderStatus::Complete {
                        100
                    } else {
                        55
                    },
                    subprocesses: vec![LoaderSubprocessSnapshot {
                        name: "seed sri capability tree".to_owned(),
                        status: runtime_status,
                    }],
                },
            ],
        }
    }
}
