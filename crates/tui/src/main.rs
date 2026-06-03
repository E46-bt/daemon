use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use carplay_protocol::{
    DspCommand, DspState, EQ_BANDS, SOCKET_PATH, ServiceMessage, Source, StatsSnapshot,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction as LayoutDirection, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Terminal,
};

fn main() -> Result<()> {
    let (msg_tx, msg_rx) = bounded::<ServiceMessage>(64);
    let (cmd_tx, cmd_rx) = bounded::<DspCommand>(32);

    // Background thread: reads ServiceMessages from the Unix socket
    thread::spawn(move || {
        loop {
            match connect_and_read(&msg_tx, &cmd_rx) {
                Ok(()) => break,
                Err(e) => {
                    eprintln!("[tui] disconnected ({}), retrying in 2s", e);
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    });

    run_tui(msg_rx, cmd_tx)
}

// Connect to the service Unix socket and bridge reads/writes until disconnected.
fn connect_and_read(msg_tx: &Sender<ServiceMessage>, cmd_rx: &Receiver<DspCommand>) -> Result<()> {
    let stream = UnixStream::connect(SOCKET_PATH)?;
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;

    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    // Forward outgoing commands to the socket in a sub-thread
    let cmd_rx = cmd_rx.clone();
    thread::spawn(move || {
        for cmd in cmd_rx {
            if let Ok(json) = serde_json::to_string(&cmd) {
                let _ = writeln!(writer, "{}", json);
            }
        }
    });

    for line in reader.lines() {
        match line {
            Ok(l) if !l.is_empty() => {
                if let Ok(msg) = serde_json::from_str::<ServiceMessage>(&l) {
                    let _ = msg_tx.send(msg);
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

// Local TUI state — mirrors DspState but adds EQ edit cursor.
struct TuiState {
    dsp: DspState,
    stats: Option<StatsSnapshot>,
    sel_band: usize,
    eq_edit: bool,
    connected: bool,
    uptime: Instant,
    frames_per_sec: u64,
}

impl TuiState {
    fn new() -> Self {
        Self {
            dsp: DspState::new(),
            stats: None,
            sel_band: 0,
            eq_edit: false,
            connected: false,
            uptime: Instant::now(),
            frames_per_sec: 0,
        }
    }

    fn apply_message(&mut self, msg: ServiceMessage) {
        match msg {
            ServiceMessage::State(s) => {
                self.dsp = s;
                self.connected = true;
            }
            ServiceMessage::Stats(s) => {
                self.frames_per_sec = s.frames_per_sec;
                self.stats = Some(s);
            }
        }
    }
}

fn run_tui(msg_rx: Receiver<ServiceMessage>, cmd_tx: Sender<DspCommand>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new();

    let result = (|| -> Result<()> {
        loop {
            // Drain all pending messages from the service
            while let Ok(msg) = msg_rx.try_recv() {
                state.apply_message(msg);
            }

            terminal.draw(|f| draw_ui(f, &state))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if let Some(cmd) = handle_key(key.code, &mut state) {
                            let _ = cmd_tx.try_send(cmd);
                        }
                        if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn handle_key(code: KeyCode, state: &mut TuiState) -> Option<DspCommand> {
    let dsp = &mut state.dsp;
    match code {
        KeyCode::Char('m') | KeyCode::Char('M') => {
            dsp.muted = !dsp.muted;
            Some(DspCommand::SetMute { value: dsp.muted })
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            dsp.volume = (dsp.volume + 0.05).min(1.0);
            Some(DspCommand::SetVolume { value: dsp.volume })
        }
        KeyCode::Char('-') => {
            dsp.volume = (dsp.volume - 0.05).max(0.0);
            Some(DspCommand::SetVolume { value: dsp.volume })
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            state.eq_edit = !state.eq_edit;
            None
        }
        KeyCode::Left if state.eq_edit => {
            state.sel_band = state.sel_band.saturating_sub(1);
            None
        }
        KeyCode::Right if state.eq_edit => {
            state.sel_band = (state.sel_band + 1).min(EQ_BANDS - 1);
            None
        }
        KeyCode::Up if state.eq_edit => {
            let g = (dsp.eq_gains[state.sel_band] + 1.0).min(12.0);
            dsp.eq_gains[state.sel_band] = g;
            Some(DspCommand::SetEqBand { band: state.sel_band, gain_db: g })
        }
        KeyCode::Down if state.eq_edit => {
            let g = (dsp.eq_gains[state.sel_band] - 1.0).max(-12.0);
            dsp.eq_gains[state.sel_band] = g;
            Some(DspCommand::SetEqBand { band: state.sel_band, gain_db: g })
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            dsp.loudness = !dsp.loudness;
            Some(DspCommand::SetLoudness { value: dsp.loudness })
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            dsp.limiter = !dsp.limiter;
            Some(DspCommand::SetLimiter { value: dsp.limiter })
        }
        KeyCode::Tab => {
            dsp.source = dsp.source.toggle();
            Some(DspCommand::SetSource { value: dsp.source })
        }
        _ => None,
    }
}

const EQ_FREQS: [f32; EQ_BANDS] = [
    31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

fn draw_ui(f: &mut ratatui::Frame, s: &TuiState) {
    let area = f.area();
    let rows = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(f, rows[0], s);

    let cols = Layout::default()
        .direction(LayoutDirection::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(rows[1]);

    draw_vu_meters(f, cols[0], s);
    draw_stats_panel(f, cols[1], s);
    draw_dsp_panel(f, rows[2], s);
    draw_footer(f, rows[3], s);
}

fn draw_header(f: &mut ratatui::Frame, area: Rect, s: &TuiState) {
    let source_style = match s.dsp.source {
        Source::Airplay => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Source::Bluetooth => Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
    };

    let active = s.stats.as_ref().map(|st| st.signal_active).unwrap_or(false);
    let clipping = s.stats.as_ref().map(|st| st.clipping).unwrap_or(false);

    let status_span = if !s.connected {
        Span::styled(" [CONNECTING...] ", Style::default().fg(Color::DarkGray))
    } else if s.dsp.muted {
        Span::styled(" [MUTED] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else if active {
        Span::styled(" [LIVE] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" [SILENCE] ", Style::default().fg(Color::DarkGray))
    };

    let clip_span = if clipping {
        Span::styled(" CLIP ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::raw("")
    };

    let up = s.uptime.elapsed().as_secs();
    let title = Paragraph::new(Line::from(vec![
        Span::styled("carplay-audio  ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("  ", Style::default().fg(Color::DarkGray)),
        Span::styled(s.dsp.source.name(), source_style),
        Span::raw("  "),
        status_span,
        clip_span,
        Span::styled(
            format!("  uptime {:02}:{:02}:{:02}", up / 3600, (up % 3600) / 60, up % 60),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM))
    .alignment(Alignment::Left);

    f.render_widget(title, area);
}

fn draw_vu_meters(f: &mut ratatui::Frame, area: Rect, s: &TuiState) {
    let block = Block::default()
        .title(" VU meters (RMS / Peak) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .margin(1)
        .split(inner);

    let (rms_l, rms_r, peak_l, peak_r) = s.stats.as_ref().map(|st| {
        (st.rms_l, st.rms_r, st.peak_l, st.peak_r)
    }).unwrap_or((0.0, 0.0, 0.0, 0.0));

    let make_gauge = |label: String, val: f32| {
        let pct = linear_to_gauge_pct(val);
        Gauge::default()
            .label(label)
            .gauge_style(Style::default().fg(vu_color(val)).bg(Color::Black))
            .ratio(pct)
    };

    f.render_widget(make_gauge(format!("L  RMS  {:>6.1} dBFS", to_dbfs(rms_l)), rms_l), rows[0]);
    f.render_widget(make_gauge(format!("L Peak  {:>6.1} dBFS", to_dbfs(peak_l)), peak_l), rows[1]);
    f.render_widget(Paragraph::new(""), rows[2]);
    f.render_widget(make_gauge(format!("R  RMS  {:>6.1} dBFS", to_dbfs(rms_r)), rms_r), rows[3]);
    f.render_widget(make_gauge(format!("R Peak  {:>6.1} dBFS", to_dbfs(peak_r)), peak_r), rows[4]);
}

fn draw_stats_panel(f: &mut ratatui::Frame, area: Rect, s: &TuiState) {
    let block = Block::default()
        .title(" Statistics ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let sr = s.dsp.source.sample_rate();
    let period = 2048u32;
    let ch = 2u32;
    let lat_ms = (period as f64 / sr as f64) * 1000.0;
    let kbps = (s.frames_per_sec as f64 * 4.0 * ch as f64) / 1024.0;
    let clipping = s.stats.as_ref().map(|st| st.clipping).unwrap_or(false);

    let text = vec![
        Line::from(vec![
            Span::styled("Format   ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}Hz / {}ch", sr, ch), Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Period   ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} frames ({:.2} ms)", period, lat_ms),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Rate     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} f/s  ({:.1} Kb/s)", s.frames_per_sec / 2, kbps),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Clip     ", Style::default().fg(Color::DarkGray)),
            if clipping {
                Span::styled("YES", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("no", Style::default().fg(Color::DarkGray))
            },
        ]),
    ];

    f.render_widget(Paragraph::new(text).block(block).wrap(Wrap { trim: false }), area);
}

fn draw_dsp_panel(f: &mut ratatui::Frame, area: Rect, s: &TuiState) {
    let lim_active = s.stats.as_ref().map(|st| st.limiter_active).unwrap_or(false);
    let dsp = &s.dsp;

    let block = Block::default()
        .title(" DSP ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(inner);

    let vol_db = if dsp.volume > 1e-9 { 20.0 * dsp.volume.log10() } else { -90.0 };
    let vol_gauge = Gauge::default()
        .label(format!("Volume  {:>5.1} dB  ({:.0}%)", vol_db, dsp.volume * 100.0))
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .ratio(dsp.volume as f64);
    f.render_widget(vol_gauge, rows[0]);

    let on = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let off = Style::default().fg(Color::DarkGray);
    let lim_style = if lim_active {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if dsp.limiter { on } else { off };

    let status = Paragraph::new(Line::from(vec![
        Span::styled("Loudness ", Style::default().fg(Color::DarkGray)),
        Span::styled(if dsp.loudness { "ON " } else { "OFF" }, if dsp.loudness { on } else { off }),
        Span::raw("    "),
        Span::styled("Limiter  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if lim_active { "ACT" } else if dsp.limiter { "ON " } else { "OFF" },
            lim_style,
        ),
    ]));
    f.render_widget(status, rows[1]);

    let band_labels: Vec<Span> = (0..EQ_BANDS)
        .map(|i| {
            let freq = EQ_FREQS[i];
            let label = if freq >= 1000.0 {
                format!("{:.0}k ", freq / 1000.0)
            } else {
                format!("{:.0}  ", freq)
            };
            let style = if s.eq_edit && i == s.sel_band {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Span::styled(label, style)
        })
        .collect();

    let gain_labels: Vec<Span> = (0..EQ_BANDS)
        .map(|i| {
            let g = dsp.eq_gains.get(i).copied().unwrap_or(0.0);
            let label = format!("{:+.0}  ", g);
            let style = if s.eq_edit && i == s.sel_band {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if g.abs() < 0.1 {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };
            Span::styled(label, style)
        })
        .collect();

    let eq_prefix = Span::styled(
        if s.eq_edit { "EQ   " } else { "EQ   " },
        Style::default().fg(if s.eq_edit { Color::Yellow } else { Color::DarkGray }),
    );

    let mut band_line = vec![eq_prefix];
    band_line.extend(band_labels);
    let mut gain_line = vec![Span::styled("     ", Style::default())];
    gain_line.extend(gain_labels);

    f.render_widget(Paragraph::new(Line::from(band_line)), rows[2]);
    f.render_widget(Paragraph::new(Line::from(gain_line)), rows[3]);
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect, s: &TuiState) {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    let sep = || Span::raw("  ");

    let source_color = match s.dsp.source {
        Source::Airplay => Color::Cyan,
        Source::Bluetooth => Color::Blue,
    };

    let mut spans = vec![
        Span::raw(" "),
        key("[Tab]"),
        Span::styled(
            format!(" {}", s.dsp.source.name()),
            Style::default().fg(source_color).add_modifier(Modifier::BOLD),
        ),
        sep(),
        key("[M]"), Span::raw(" Mute"),
        sep(),
        key("[+/-]"), Span::raw(" Volume"),
        sep(),
        key("[L]"), Span::raw(" Loudness"),
        sep(),
        key("[R]"), Span::raw(" Limiter"),
        sep(),
        key("[E]"), Span::raw(if s.eq_edit { " EQ  / " } else { " EQ" }),
        sep(),
        key("[Q]"), Span::raw(" Quit"),
    ];

    if s.eq_edit {
        spans.push(sep());
        spans.push(Span::styled(
            "  EQ mode:  /  band   /  gain",
            Style::default().fg(Color::Yellow),
        ));
    }

    let footer = Paragraph::new(Line::from(spans))
        .block(Block::default().borders(Borders::TOP))
        .alignment(Alignment::Left);

    f.render_widget(footer, area);
}

fn linear_to_gauge_pct(linear: f32) -> f64 {
    if linear < 1e-9 { return 0.0; }
    let db = 20.0f64 * (linear as f64).log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn vu_color(linear: f32) -> Color {
    let db = to_dbfs(linear);
    if db >= -3.0 { Color::Red } else if db >= -12.0 { Color::Yellow } else { Color::Green }
}

fn to_dbfs(linear: f32) -> f32 {
    if linear < 1e-9 { -90.0 } else { 20.0 * linear.log10() }
}
