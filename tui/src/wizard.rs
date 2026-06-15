use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agentbox_core::config::{
    AgentId, BoxConfig, FolderConfig, Lifecycle, NetworkMode, ProviderConfig, ProviderType,
    ResourceConfig, SyncMode,
};
use agentbox_core::container::{BoxInfo, ContainerStatus};
use agentbox_core::manifest;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};

// ── Themes ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct ThemeColors {
    pub name: &'static str,
    pub border: Color,
    pub border_focused: Color,
    pub border_header: Color,
    pub text: Color,
    pub text_dim: Color,
    pub selection: Color,
    pub running: Color,
    pub stopped: Color,
    pub orphaned: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub new_item: Color,
    pub cursor_bg: Color,
    pub cursor_fg: Color,
}

const THEMES: [ThemeColors; 5] = [
    // 0 · Dark (default — deep navy)
    ThemeColors {
        name: "Dark",
        border: Color::Rgb(46, 50, 69),
        border_focused: Color::Rgb(120, 160, 255),
        border_header: Color::Rgb(94, 147, 242),
        text: Color::Rgb(208, 214, 240),
        text_dim: Color::Rgb(107, 116, 148),
        selection: Color::Rgb(120, 160, 255),
        running: Color::Rgb(76, 175, 125),
        stopped: Color::Rgb(107, 116, 148),
        orphaned: Color::Rgb(224, 92, 92),
        accent: Color::Rgb(94, 147, 242),
        success: Color::Rgb(76, 175, 125),
        error: Color::Rgb(224, 92, 92),
        new_item: Color::Rgb(94, 147, 242),
        cursor_bg: Color::Rgb(120, 160, 255),
        cursor_fg: Color::Black,
    },
    // 1 · Dracula
    ThemeColors {
        name: "Dracula",
        border: Color::Rgb(68, 71, 90),
        border_focused: Color::Rgb(189, 147, 249),
        border_header: Color::Rgb(255, 121, 198),
        text: Color::Rgb(248, 248, 242),
        text_dim: Color::Rgb(98, 114, 164),
        selection: Color::Rgb(189, 147, 249),
        running: Color::Rgb(80, 250, 123),
        stopped: Color::Rgb(98, 114, 164),
        orphaned: Color::Rgb(255, 85, 85),
        accent: Color::Rgb(139, 233, 253),
        success: Color::Rgb(80, 250, 123),
        error: Color::Rgb(255, 85, 85),
        new_item: Color::Rgb(139, 233, 253),
        cursor_bg: Color::Rgb(189, 147, 249),
        cursor_fg: Color::Black,
    },
    // 2 · Nord
    ThemeColors {
        name: "Nord",
        border: Color::Rgb(59, 66, 82),
        border_focused: Color::Rgb(136, 192, 208),
        border_header: Color::Rgb(94, 129, 172),
        text: Color::Rgb(236, 239, 244),
        text_dim: Color::Rgb(76, 86, 106),
        selection: Color::Rgb(136, 192, 208),
        running: Color::Rgb(163, 190, 140),
        stopped: Color::Rgb(76, 86, 106),
        orphaned: Color::Rgb(191, 97, 106),
        accent: Color::Rgb(129, 161, 193),
        success: Color::Rgb(163, 190, 140),
        error: Color::Rgb(191, 97, 106),
        new_item: Color::Rgb(129, 161, 193),
        cursor_bg: Color::Rgb(136, 192, 208),
        cursor_fg: Color::Black,
    },
    // 3 · Catppuccin Mocha
    ThemeColors {
        name: "Catppuccin",
        border: Color::Rgb(88, 91, 112),
        border_focused: Color::Rgb(180, 190, 254),
        border_header: Color::Rgb(245, 194, 231),
        text: Color::Rgb(205, 214, 244),
        text_dim: Color::Rgb(88, 91, 112),
        selection: Color::Rgb(180, 190, 254),
        running: Color::Rgb(166, 227, 161),
        stopped: Color::Rgb(88, 91, 112),
        orphaned: Color::Rgb(243, 139, 168),
        accent: Color::Rgb(137, 180, 250),
        success: Color::Rgb(166, 227, 161),
        error: Color::Rgb(243, 139, 168),
        new_item: Color::Rgb(148, 226, 213),
        cursor_bg: Color::Rgb(180, 190, 254),
        cursor_fg: Color::Black,
    },
    // 4 · Gruvbox Dark
    ThemeColors {
        name: "Gruvbox",
        border: Color::Rgb(80, 73, 69),
        border_focused: Color::Rgb(215, 153, 33),
        border_header: Color::Rgb(152, 151, 26),
        text: Color::Rgb(235, 219, 178),
        text_dim: Color::Rgb(124, 111, 100),
        selection: Color::Rgb(215, 153, 33),
        running: Color::Rgb(184, 187, 38),
        stopped: Color::Rgb(124, 111, 100),
        orphaned: Color::Rgb(251, 73, 52),
        accent: Color::Rgb(131, 165, 152),
        success: Color::Rgb(184, 187, 38),
        error: Color::Rgb(251, 73, 52),
        new_item: Color::Rgb(131, 165, 152),
        cursor_bg: Color::Rgb(215, 153, 33),
        cursor_fg: Color::Black,
    },
];

// ── Public surface ────────────────────────────────────────────────────────────

pub enum WizardResult {
    Cancelled,
    Launch {
        config: Box<BoxConfig>,
        manifests_dir: Option<PathBuf>,
    },
    Attach {
        box_name: String,
    },
}

pub fn run() -> anyhow::Result<WizardResult> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ── Screens ───────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Home,      // list existing boxes + "New box"
    BoxDetail, // manage a selected box
    Images,    // cached agent images
    WizardAgent,
    WizardFolder,
    WizardLifecycle,
    WizardProvider,
    WizardPiModels, // Pi-only: multi-model JSON config
    WizardSummary,
}

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AgentEntry {
    id: String,
    display_name: String,
    is_daemon: bool,
}

/// Simple text-input with a char-level cursor.
struct Input {
    chars: Vec<char>,
    cursor: usize,
}

impl Input {
    fn new(s: &str) -> Self {
        let chars: Vec<char> = s.chars().collect();
        let cursor = chars.len();
        Self { chars, cursor }
    }
    fn value(&self) -> String {
        self.chars.iter().collect()
    }
    fn on_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Char(c) => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            KeyCode::Left if self.cursor > 0 => self.cursor -= 1,
            KeyCode::Right if self.cursor < self.chars.len() => self.cursor += 1,
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => {}
        }
    }
    fn widget<'a>(&self, title: &'a str, focused: bool, theme: &ThemeColors) -> Paragraph<'a> {
        let border_style = if focused {
            Style::default().fg(theme.border_focused)
        } else {
            Style::default().fg(theme.border)
        };
        let content = if focused {
            let before: String = self.chars[..self.cursor].iter().collect();
            let (cur, after): (String, String) = if self.cursor < self.chars.len() {
                (
                    self.chars[self.cursor].to_string(),
                    self.chars[self.cursor + 1..].iter().collect(),
                )
            } else {
                (" ".to_string(), String::new())
            };
            Text::from(Line::from(vec![
                Span::raw(before),
                Span::styled(
                    cur,
                    Style::default().bg(theme.cursor_bg).fg(theme.cursor_fg),
                ),
                Span::raw(after),
            ]))
        } else {
            Text::from(self.value())
        };
        Paragraph::new(content).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
    }
}

/// Simple multi-line text editor for JSON input (Pi models.json).
struct MultilineInput {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    scroll: usize,
}

impl MultilineInput {
    fn new(s: &str) -> Self {
        let lines: Vec<Vec<char>> = if s.is_empty() {
            vec![vec![]]
        } else {
            s.lines().map(|l| l.chars().collect()).collect()
        };
        let row = lines.len().saturating_sub(1);
        let col = lines.last().map(|l| l.len()).unwrap_or(0);
        Self {
            lines,
            row,
            col,
            scroll: 0,
        }
    }

    fn value(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn on_key(&mut self, key: &KeyEvent, vis_rows: usize) {
        match key.code {
            KeyCode::Char(c) => {
                self.lines[self.row].insert(self.col, c);
                self.col += 1;
            }
            KeyCode::Enter => {
                let rest = self.lines[self.row].split_off(self.col);
                self.row += 1;
                self.lines.insert(self.row, rest);
                self.col = 0;
            }
            KeyCode::Backspace if self.col > 0 => {
                self.col -= 1;
                self.lines[self.row].remove(self.col);
            }
            KeyCode::Backspace if self.row > 0 => {
                let row = self.lines.remove(self.row);
                self.row -= 1;
                self.col = self.lines[self.row].len();
                self.lines[self.row].extend(row);
            }
            KeyCode::Left if self.col > 0 => self.col -= 1,
            KeyCode::Left if self.row > 0 => {
                self.row -= 1;
                self.col = self.lines[self.row].len();
            }
            KeyCode::Right if self.col < self.lines[self.row].len() => self.col += 1,
            KeyCode::Right if self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = 0;
            }
            KeyCode::Up if self.row > 0 => {
                self.row -= 1;
                self.col = self.col.min(self.lines[self.row].len());
            }
            KeyCode::Down if self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = self.col.min(self.lines[self.row].len());
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.lines[self.row].len(),
            _ => {}
        }
        // Keep scroll in sync with cursor
        if self.row < self.scroll {
            self.scroll = self.row;
        }
        if vis_rows > 0 && self.row >= self.scroll + vis_rows {
            self.scroll = self.row + 1 - vis_rows;
        }
    }

    fn widget<'a>(&self, title: &'a str, theme: &ThemeColors) -> Paragraph<'a> {
        let lines: Vec<Line> = self
            .lines
            .iter()
            .enumerate()
            .skip(self.scroll)
            .map(|(r, chars)| {
                if r == self.row {
                    let before: String = chars[..self.col].iter().collect();
                    let (cur, after): (String, String) = if self.col < chars.len() {
                        (
                            chars[self.col].to_string(),
                            chars[self.col + 1..].iter().collect(),
                        )
                    } else {
                        (" ".to_string(), String::new())
                    };
                    Line::from(vec![
                        Span::raw(before),
                        Span::styled(
                            cur,
                            Style::default().bg(theme.cursor_bg).fg(theme.cursor_fg),
                        ),
                        Span::raw(after),
                    ])
                } else {
                    Line::from(chars.iter().collect::<String>())
                }
            })
            .collect();

        Paragraph::new(lines).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border_focused)),
        )
    }
}

const PROVIDER_TYPES: [ProviderType; 3] = [
    ProviderType::Anthropic,
    ProviderType::Openai,
    ProviderType::OpenaiCompatible,
];

fn pt_label(pt: &ProviderType) -> &'static str {
    match pt {
        ProviderType::Anthropic => "anthropic",
        ProviderType::Openai => "openai",
        ProviderType::OpenaiCompatible => "openai-compatible",
    }
}

fn default_auth(pt: &ProviderType) -> &'static str {
    match pt {
        ProviderType::Anthropic => "${env:ANTHROPIC_API_KEY}",
        ProviderType::Openai | ProviderType::OpenaiCompatible => "${env:OPENAI_API_KEY}",
    }
}

// ── Box detail actions ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum BoxAction {
    Attach,
    Stop,
    Kill,
    Remove,
}

const PERSISTENT_ACTIONS: [(BoxAction, &str); 3] = [
    (BoxAction::Attach, "Attach  (reconnect and launch agent)"),
    (BoxAction::Stop, "Stop    (halt container, keep state)"),
    (
        BoxAction::Remove,
        "Remove  (delete container + state volume)",
    ),
];

const DAEMON_ACTIONS: [(BoxAction, &str); 3] = [
    (
        BoxAction::Attach,
        "Status  (show running state + bound ports)",
    ),
    (BoxAction::Stop, "Stop    (halt daemon, preserve state)"),
    (
        BoxAction::Remove,
        "Remove  (delete container + state volume)",
    ),
];

const EPHEMERAL_ACTIONS: [(BoxAction, &str); 1] =
    [(BoxAction::Kill, "Kill    (force-remove orphaned container)")];

// ── App state ─────────────────────────────────────────────────────────────────

struct App {
    screen: Screen,

    // Home screen
    boxes: Vec<BoxInfo>,
    boxes_idx: usize,
    boxes_list_state: ListState,

    // Box detail screen
    detail_box: Option<BoxInfo>,
    detail_action_idx: usize,
    detail_list_state: ListState,
    detail_stats: Option<agentbox_core::ContainerStats>,
    stats_last_tick: Instant,

    // Images screen
    cache_images: Vec<agentbox_core::CacheImage>,
    images_idx: usize,
    images_list_state: ListState,

    // Wizard — agent
    agents: Vec<AgentEntry>,
    agent_idx: usize,
    agent_list_state: ListState,

    // Wizard — folder
    folder: Input,
    folder_err: Option<String>,
    project_name: Input,
    folder_focus: usize, // 0=folder, 1=project_name

    // Wizard — lifecycle
    lifecycle_idx: usize, // 0=ephemeral, 1=persistent
    box_name: Input,

    // Wizard — provider
    prov_type_idx: usize,
    prov_name: Input,
    prov_model: Input,
    prov_base_url: Input,
    prov_auth: Input,
    prov_focus: usize, // 0-4

    // Wizard — Pi models.json (only shown when agent is Pi)
    pi_models: MultilineInput,
    pi_models_err: Option<String>,

    // Status / error
    status_msg: Option<(String, bool)>, // (message, is_error)

    manifests_dir: Option<PathBuf>,

    // Visual theme (cycles with Ctrl+T)
    theme_idx: usize,
}

impl App {
    fn new() -> Self {
        let manifests_dir = find_manifests_dir();

        // Load existing boxes
        let boxes = do_async(agentbox_core::list_boxes()).unwrap_or_default();
        let mut boxes_list_state = ListState::default();
        if !boxes.is_empty() {
            boxes_list_state.select(Some(0));
        }

        // Gather available agents
        let manifest_agents: Vec<AgentEntry> = manifests_dir
            .as_deref()
            .map(manifest::list_manifests_meta)
            .unwrap_or_default()
            .into_iter()
            .map(|m| AgentEntry {
                id: m.id,
                display_name: m.display_name,
                is_daemon: m.is_daemon,
            })
            .collect();
        let manifest_ids: std::collections::HashSet<&str> =
            manifest_agents.iter().map(|a| a.id.as_str()).collect();
        let builtins: Vec<AgentEntry> = [("claude-code", "Claude Code"), ("opencode", "OpenCode")]
            .iter()
            .filter(|(id, _)| !manifest_ids.contains(*id))
            .map(|(id, name)| AgentEntry {
                id: id.to_string(),
                display_name: name.to_string(),
                is_daemon: false,
            })
            .collect();
        let agents: Vec<AgentEntry> = manifest_agents.into_iter().chain(builtins).collect();

        let mut agent_list_state = ListState::default();
        agent_list_state.select(Some(0));

        let mut detail_list_state = ListState::default();
        detail_list_state.select(Some(0));

        let cache_images = do_async(agentbox_core::list_cache_images()).unwrap_or_default();
        let mut images_list_state = ListState::default();
        if !cache_images.is_empty() {
            images_list_state.select(Some(0));
        }

        App {
            screen: Screen::Home,
            boxes,
            boxes_idx: 0,
            boxes_list_state,

            detail_box: None,
            detail_action_idx: 0,
            detail_list_state,
            detail_stats: None,
            stats_last_tick: Instant::now() - Duration::from_secs(60),

            cache_images,
            images_idx: 0,
            images_list_state,

            agents,
            agent_idx: 0,
            agent_list_state,

            folder: Input::new(""),
            folder_err: None,
            project_name: Input::new(""),
            folder_focus: 0,

            lifecycle_idx: 0,
            box_name: Input::new(""),

            prov_type_idx: 0,
            prov_name: Input::new(""),
            prov_model: Input::new(""),
            prov_base_url: Input::new(""),
            prov_auth: Input::new(default_auth(&PROVIDER_TYPES[0])),
            prov_focus: 0,

            pi_models: MultilineInput::new(""),
            pi_models_err: None,

            status_msg: None,
            manifests_dir,

            theme_idx: 0,
        }
    }

    fn theme(&self) -> &'static ThemeColors {
        &THEMES[self.theme_idx]
    }

    fn current_provider_type(&self) -> &ProviderType {
        &PROVIDER_TYPES[self.prov_type_idx]
    }

    fn is_persistent(&self) -> bool {
        self.lifecycle_idx == 1
    }

    fn selected_agent_id(&self) -> &str {
        self.agents
            .get(self.agent_idx)
            .map(|a| a.id.as_str())
            .unwrap_or("")
    }

    fn selected_agent_is_daemon(&self) -> bool {
        self.agents
            .get(self.agent_idx)
            .map(|a| a.is_daemon)
            .unwrap_or(false)
    }

    fn is_pi(&self) -> bool {
        self.selected_agent_id() == "pi"
    }

    fn build_config(&self) -> BoxConfig {
        let pt = self.current_provider_type().clone();
        let base_url = if pt == ProviderType::OpenaiCompatible {
            let s = self.prov_base_url.value();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        };
        let name = if self.is_persistent() {
            let n = self.box_name.value();
            if n.is_empty() {
                None
            } else {
                Some(n)
            }
        } else {
            None
        };
        BoxConfig {
            agent: AgentId(self.agents[self.agent_idx].id.clone()),
            name,
            project_name: {
                let pn = self.project_name.value();
                if pn.trim().is_empty() {
                    None
                } else {
                    Some(pn)
                }
            },
            folder: FolderConfig {
                path: PathBuf::from(self.folder.value()),
                sync: SyncMode::Mount,
            },
            lifecycle: if self.is_persistent() {
                Lifecycle::Persistent
            } else {
                Lifecycle::Ephemeral
            },
            provider: ProviderConfig {
                name: if self.prov_name.value().is_empty() {
                    pt_label(&pt).to_string()
                } else {
                    self.prov_name.value()
                },
                provider_type: pt,
                model: self.prov_model.value(),
                base_url,
                auth: self.prov_auth.value(),
                raw: {
                    let s = self.pi_models.value();
                    if self.is_pi() && !s.trim().is_empty() {
                        serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
                    } else {
                        serde_json::Value::Null
                    }
                },
            },
            network: NetworkMode::Open,
            resources: ResourceConfig {
                cpus: None,
                memory: None,
            },
            extra_env: HashMap::new(),
            backend: agentbox_core::config::BackendChoice::Auto,
            hooks: Default::default(),
            extra_mounts: vec![],
            notifications: false,
            remote: None,
        }
    }

    fn refresh_boxes(&mut self) {
        self.boxes = do_async(agentbox_core::list_boxes()).unwrap_or_default();
        if self.boxes.is_empty() {
            self.boxes_list_state.select(None);
            self.boxes_idx = 0;
        } else {
            let idx = self.boxes_idx.min(self.boxes.len() - 1);
            self.boxes_idx = idx;
            self.boxes_list_state.select(Some(idx));
        }
    }

    fn refresh_images(&mut self) {
        self.cache_images = do_async(agentbox_core::list_cache_images()).unwrap_or_default();
        if self.cache_images.is_empty() {
            self.images_list_state.select(None);
            self.images_idx = 0;
        } else {
            let idx = self.images_idx.min(self.cache_images.len() - 1);
            self.images_idx = idx;
            self.images_list_state.select(Some(idx));
        }
    }

    fn detail_actions(&self) -> &[(BoxAction, &'static str)] {
        let b = match self.detail_box.as_ref() {
            Some(b) => b,
            None => return &PERSISTENT_ACTIONS,
        };
        if b.lifecycle == "ephemeral" {
            &EPHEMERAL_ACTIONS
        } else if b.is_daemon {
            &DAEMON_ACTIONS
        } else {
            &PERSISTENT_ACTIONS
        }
    }
}

// ── Async helper (for use inside spawn_blocking) ──────────────────────────────

fn do_async<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(f)
}

// ── Event loop ────────────────────────────────────────────────────────────────

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<WizardResult> {
    loop {
        // Refresh CPU/MEM stats every 2s when viewing a running non-daemon box.
        if app.screen == Screen::BoxDetail {
            let now = Instant::now();
            if now.duration_since(app.stats_last_tick) >= Duration::from_secs(2) {
                app.stats_last_tick = now;
                if let Some(b) = app
                    .detail_box
                    .as_ref()
                    .filter(|b| b.status == ContainerStatus::Running && !b.is_daemon)
                {
                    let name = format!("agentbox-{}", b.box_name);
                    app.detail_stats = do_async(agentbox_core::get_container_stats(&name)).ok();
                }
            }
        } else {
            app.detail_stats = None;
        }

        terminal.draw(|f| render(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(WizardResult::Cancelled);
                }
                // Ctrl+T: cycle color theme globally from any screen
                if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.theme_idx = (app.theme_idx + 1) % THEMES.len();
                    app.status_msg =
                        Some((format!("Theme: {}", THEMES[app.theme_idx].name), false));
                    continue;
                }
                match handle_key(app, key) {
                    Action::Continue => {}
                    Action::Quit => return Ok(WizardResult::Cancelled),
                    Action::Launch => {
                        return Ok(WizardResult::Launch {
                            config: Box::new(app.build_config()),
                            manifests_dir: app.manifests_dir.clone(),
                        });
                    }
                    Action::Attach(box_name) => {
                        return Ok(WizardResult::Attach { box_name });
                    }
                }
            }
        }
    }
}

enum Action {
    Continue,
    Quit,
    Launch,
    Attach(String),
}

fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    match app.screen {
        Screen::Home => handle_home(app, key),
        Screen::BoxDetail => handle_box_detail(app, key),
        Screen::Images => handle_images(app, key),
        Screen::WizardAgent => handle_agent(app, key),
        Screen::WizardFolder => handle_folder(app, key),
        Screen::WizardLifecycle => handle_lifecycle(app, key),
        Screen::WizardProvider => handle_provider(app, key),
        Screen::WizardPiModels => handle_pi_models(app, key),
        Screen::WizardSummary => handle_summary(app, key),
    }
}

// ── Home ──────────────────────────────────────────────────────────────────────

fn handle_home(app: &mut App, key: KeyEvent) -> Action {
    // "New box" is the last item in the list (after all boxes)
    let total = app.boxes.len() + 1; // boxes + "New box"
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
        KeyCode::Up | KeyCode::Char('k') => {
            if app.boxes_idx > 0 {
                app.boxes_idx -= 1;
                app.boxes_list_state.select(Some(app.boxes_idx));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.boxes_idx + 1 < total {
                app.boxes_idx += 1;
                app.boxes_list_state.select(Some(app.boxes_idx));
            }
        }
        KeyCode::Enter => {
            if app.boxes_idx < app.boxes.len() {
                // Open box detail
                app.detail_box = Some(app.boxes[app.boxes_idx].clone());
                app.detail_action_idx = 0;
                app.detail_list_state.select(Some(0));
                app.screen = Screen::BoxDetail;
            } else {
                // New box wizard
                app.screen = Screen::WizardAgent;
            }
        }
        KeyCode::Char('n') => {
            app.screen = Screen::WizardAgent;
        }
        KeyCode::Char('r') => {
            app.refresh_boxes();
        }
        KeyCode::Char('i') => {
            app.refresh_images();
            app.screen = Screen::Images;
        }
        _ => {}
    }
    Action::Continue
}

// ── Box detail ────────────────────────────────────────────────────────────────

fn handle_box_detail(app: &mut App, key: KeyEvent) -> Action {
    let actions_len = app.detail_actions().len();
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::Home;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.detail_action_idx > 0 {
                app.detail_action_idx -= 1;
                app.detail_list_state.select(Some(app.detail_action_idx));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.detail_action_idx + 1 < actions_len {
                app.detail_action_idx += 1;
                app.detail_list_state.select(Some(app.detail_action_idx));
            }
        }
        KeyCode::Enter => {
            let box_name = app
                .detail_box
                .as_ref()
                .map(|b| b.box_name.clone())
                .unwrap_or_default();
            let idx = app.detail_action_idx.min(actions_len.saturating_sub(1));
            let action = app.detail_actions()[idx].0;
            match action {
                BoxAction::Attach => return Action::Attach(box_name),
                BoxAction::Stop => {
                    let result = do_async(agentbox_core::stop_box(&box_name));
                    app.status_msg = Some(match result {
                        Ok(()) => (format!("Box '{box_name}' stopped."), false),
                        Err(e) => (format!("Error: {e}"), true),
                    });
                    app.refresh_boxes();
                    app.screen = Screen::Home;
                }
                BoxAction::Kill => {
                    let result = do_async(agentbox_core::kill_box(&box_name));
                    app.status_msg = Some(match result {
                        Ok(()) => (format!("Container '{box_name}' force-removed."), false),
                        Err(e) => (format!("Error: {e}"), true),
                    });
                    app.refresh_boxes();
                    app.screen = Screen::Home;
                }
                BoxAction::Remove => {
                    let result = do_async(agentbox_core::remove_box(&box_name));
                    app.status_msg = Some(match result {
                        Ok(()) => (format!("Box '{box_name}' removed."), false),
                        Err(e) => (format!("Error: {e}"), true),
                    });
                    app.refresh_boxes();
                    app.screen = Screen::Home;
                }
            }
        }
        _ => {}
    }
    Action::Continue
}

// ── Images ────────────────────────────────────────────────────────────────────

fn handle_images(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.screen = Screen::Home;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.images_idx > 0 {
                app.images_idx -= 1;
                app.images_list_state.select(Some(app.images_idx));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.images_idx + 1 < app.cache_images.len() {
                app.images_idx += 1;
                app.images_list_state.select(Some(app.images_idx));
            }
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if let Some(img) = app.cache_images.get(app.images_idx) {
                let agent_id = img.agent_id.clone();
                let result = do_async(agentbox_core::remove_cache_image(&agent_id));
                app.status_msg = Some(match result {
                    Ok(()) => (format!("Cache image for '{agent_id}' removed."), false),
                    Err(e) => (format!("Error: {e}"), true),
                });
                app.refresh_images();
            }
        }
        KeyCode::Char('r') => {
            app.refresh_images();
        }
        _ => {}
    }
    Action::Continue
}

// ── Wizard: agent ─────────────────────────────────────────────────────────────

fn handle_agent(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::Home;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.agent_idx > 0 {
                app.agent_idx -= 1;
                app.agent_list_state.select(Some(app.agent_idx));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.agent_idx + 1 < app.agents.len() {
                app.agent_idx += 1;
                app.agent_list_state.select(Some(app.agent_idx));
            }
        }
        KeyCode::Enter => {
            app.screen = Screen::WizardFolder;
        }
        _ => {}
    }
    Action::Continue
}

// ── Wizard: folder ────────────────────────────────────────────────────────────

fn handle_folder(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.folder_err = None;
            app.folder_focus = 0;
            app.screen = Screen::WizardAgent;
        }
        KeyCode::Tab => {
            app.folder_focus = if app.folder_focus == 0 { 1 } else { 0 };
        }
        KeyCode::Enter if app.folder_focus == 0 => {
            let path = PathBuf::from(app.folder.value());
            if !path.exists() {
                app.folder_err = Some(format!("Path does not exist: {}", path.display()));
            } else if !path.is_dir() {
                app.folder_err = Some("Path is not a directory".into());
            } else {
                app.folder_err = None;
                // Auto-fill project name from basename if not yet set
                if app.project_name.value().is_empty() {
                    if let Some(basename) = path.file_name() {
                        app.project_name = Input::new(&basename.to_string_lossy());
                    }
                }
                app.folder_focus = 1;
            }
        }
        KeyCode::Enter if app.folder_focus == 1 => {
            let path = PathBuf::from(app.folder.value());
            // Daemon agents require persistent lifecycle.
            if app.selected_agent_is_daemon() {
                app.lifecycle_idx = 1;
            }
            // Pre-fill box name suggestion
            if app.box_name.value().is_empty() {
                let agent_id = app.selected_agent_id().to_string();
                let basename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !agent_id.is_empty() && !basename.is_empty() {
                    app.box_name = Input::new(&format!("{agent_id}-{basename}"));
                }
            }
            app.folder_focus = 0;
            app.screen = Screen::WizardLifecycle;
        }
        _ => {
            if app.folder_focus == 0 {
                app.folder.on_key(&key);
                app.folder_err = None;
            } else {
                app.project_name.on_key(&key);
            }
        }
    }
    Action::Continue
}

// ── Wizard: lifecycle ─────────────────────────────────────────────────────────

fn handle_lifecycle(app: &mut App, key: KeyEvent) -> Action {
    let daemon_locked = app.selected_agent_is_daemon();
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::WizardFolder;
        }
        KeyCode::Left if app.lifecycle_idx > 0 && app.prov_focus == 0 && !daemon_locked => {
            app.lifecycle_idx -= 1;
        }
        KeyCode::Right if app.lifecycle_idx < 1 && app.prov_focus == 0 && !daemon_locked => {
            app.lifecycle_idx += 1;
        }
        KeyCode::Tab => {
            if app.is_persistent() {
                app.prov_focus = if app.prov_focus == 0 { 1 } else { 0 };
            }
        }
        KeyCode::Enter => {
            if app.is_persistent() && app.prov_focus == 0 {
                // Advance to name field
                app.prov_focus = 1;
            } else if app.is_persistent() && app.box_name.value().is_empty() {
                // require a name
            } else {
                app.prov_focus = 0;
                app.screen = Screen::WizardProvider;
            }
        }
        _ => {
            if app.is_persistent() && app.prov_focus == 1 {
                app.box_name.on_key(&key);
            }
        }
    }
    Action::Continue
}

// ── Wizard: provider ──────────────────────────────────────────────────────────

fn handle_provider(app: &mut App, key: KeyEvent) -> Action {
    let focus = app.prov_focus;
    match key.code {
        KeyCode::Esc => {
            app.prov_focus = 0;
            app.screen = Screen::WizardLifecycle;
        }
        KeyCode::Tab => {
            app.prov_focus = (app.prov_focus + 1) % 5;
        }
        KeyCode::BackTab => {
            app.prov_focus = if app.prov_focus == 0 {
                4
            } else {
                app.prov_focus - 1
            };
        }
        KeyCode::Enter if focus == 4 => {
            app.prov_focus = 0;
            app.screen = if app.is_pi() {
                Screen::WizardPiModels
            } else {
                Screen::WizardSummary
            };
        }
        KeyCode::Enter => {
            app.prov_focus = (focus + 1) % 5;
        }
        KeyCode::Left if focus == 0 && app.prov_type_idx > 0 => {
            app.prov_type_idx -= 1;
            app.prov_auth = Input::new(default_auth(app.current_provider_type()));
        }
        KeyCode::Right if focus == 0 && app.prov_type_idx + 1 < PROVIDER_TYPES.len() => {
            app.prov_type_idx += 1;
            app.prov_auth = Input::new(default_auth(app.current_provider_type()));
        }
        _ => match focus {
            1 => app.prov_name.on_key(&key),
            2 => app.prov_model.on_key(&key),
            3 => app.prov_base_url.on_key(&key),
            4 => app.prov_auth.on_key(&key),
            _ => {}
        },
    }
    Action::Continue
}

// ── Wizard: Pi models ─────────────────────────────────────────────────────────

fn handle_pi_models(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.pi_models_err = None;
            app.screen = Screen::WizardProvider;
        }
        // F5 → advance (plain Enter inserts newlines in the JSON editor)
        KeyCode::F(5) => {
            let txt = app.pi_models.value();
            if !txt.trim().is_empty() {
                if let Err(e) = serde_json::from_str::<serde_json::Value>(&txt) {
                    app.pi_models_err = Some(format!("JSON error: {e}"));
                    return Action::Continue;
                }
            }
            app.pi_models_err = None;
            app.screen = Screen::WizardSummary;
        }
        _ => app.pi_models.on_key(&key, 20), // rough vis estimate; scroll stays conservative
    }
    Action::Continue
}

// ── Wizard: summary ───────────────────────────────────────────────────────────

fn handle_summary(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::WizardProvider;
        }
        KeyCode::Enter => return Action::Launch,
        KeyCode::Char('q') => return Action::Quit,
        _ => {}
    }
    Action::Continue
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(frame.area());

    render_header(frame, chunks[0], app);
    match app.screen {
        Screen::Home => render_home(frame, chunks[1], app),
        Screen::BoxDetail => render_box_detail(frame, chunks[1], app),
        Screen::Images => render_images(frame, chunks[1], app),
        Screen::WizardAgent => render_agent(frame, chunks[1], app),
        Screen::WizardFolder => render_folder(frame, chunks[1], app),
        Screen::WizardLifecycle => render_lifecycle(frame, chunks[1], app),
        Screen::WizardProvider => render_provider(frame, chunks[1], app),
        Screen::WizardPiModels => render_pi_models(frame, chunks[1], app),
        Screen::WizardSummary => render_summary(frame, chunks[1], app),
    }
    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let title = match app.screen {
        Screen::Home => " agentbox ",
        Screen::BoxDetail => " agentbox — manage box ",
        Screen::Images => " agentbox — cache images ",
        Screen::WizardAgent => " agentbox — new box (1/5) select agent ",
        Screen::WizardFolder => " agentbox — new box (2/5) workspace folder ",
        Screen::WizardLifecycle => " agentbox — new box (3/5) lifecycle ",
        Screen::WizardProvider => " agentbox — new box (4/5) provider ",
        Screen::WizardPiModels => " agentbox — new box (4b) Pi custom models (optional) ",
        Screen::WizardSummary => " agentbox — new box (5/5) ready to launch ",
    };
    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border_header).bold());
    frame.render_widget(block, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let base_help = match app.screen {
        Screen::Home => "↑↓ navigate  Enter open  n new box  i images  r refresh  q quit",
        Screen::BoxDetail => "↑↓ select action  Enter execute  Esc back",
        Screen::Images => "↑↓ navigate  d delete  r refresh  Esc back",
        Screen::WizardAgent => "↑↓/jk navigate  Enter select  Esc back",
        Screen::WizardFolder => "Type path  Enter confirm  Tab project name  Esc back",
        Screen::WizardLifecycle => "← → type  Tab name field  Enter next  Esc back",
        Screen::WizardProvider => "Tab/↵ next field  ← → type (field 1)  Esc back",
        Screen::WizardPiModels => "Type JSON  F5 continue  Esc back  (leave empty to skip)",
        Screen::WizardSummary => "Enter launch  Esc back  q quit",
    };
    let help = format!("{base_help}  │  Ctrl+T theme ({})", t.name);

    let content = if let Some((msg, is_err)) = &app.status_msg {
        let style = if *is_err {
            Style::default().fg(t.error)
        } else {
            Style::default().fg(t.success)
        };
        Paragraph::new(msg.as_str())
            .style(style)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(t.border)),
            )
    } else {
        Paragraph::new(help.as_str())
            .style(Style::default().fg(t.text_dim))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(t.border)),
            )
    };

    frame.render_widget(content, area);
}

fn render_logo(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let c = t.accent;
    let d = t.text_dim;
    let w = t.text;

    // Robot eyes — bright white
    let eye = Style::default().fg(Color::Rgb(220, 235, 255)).bold();
    let art = vec![
        Line::from(vec![
            Span::styled("       ", Style::default()),
            Span::styled("·:·:·", Style::default().fg(c)),
        ]),
        Line::from(vec![
            Span::styled("        ", Style::default()),
            Span::styled("|||", Style::default().fg(d)),
        ]),
        Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled("╔═════════╗", Style::default().fg(c)),
            Span::styled("   ", Style::default()),
            Span::styled("a g e n t b o x", Style::default().fg(c).bold()),
        ]),
        Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled("║ ", Style::default().fg(c)),
            Span::styled("◉", eye),
            Span::styled("     ", Style::default()),
            Span::styled("◉", eye),
            Span::styled(" ║", Style::default().fg(c)),
            Span::styled("   ", Style::default()),
            Span::styled("────────────────────────", Style::default().fg(d)),
        ]),
        Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled("║  ", Style::default().fg(c)),
            Span::styled("─────", Style::default().fg(d)),
            Span::styled("  ║", Style::default().fg(c)),
            Span::styled("   ", Style::default()),
            Span::styled(
                "run AI agents in isolated containers",
                Style::default().fg(w),
            ),
        ]),
        Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled("╚═════╤═══╝", Style::default().fg(c)),
        ]),
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("╔═══════╧═════╗", Style::default().fg(c)),
        ]),
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("║             ║", Style::default().fg(c)),
        ]),
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("╚═════════════╝", Style::default().fg(c)),
        ]),
    ];

    let logo = Paragraph::new(art)
        .block(Block::default().borders(Borders::NONE))
        .alignment(Alignment::Left);
    frame.render_widget(logo, area);
}

fn render_home(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let chunks = Layout::vertical([Constraint::Length(11), Constraint::Min(0)]).split(area);
    render_logo(frame, chunks[0], app);
    let area = chunks[1];
    let total = app.boxes.len() + 1;
    let mut items: Vec<ListItem> = app
        .boxes
        .iter()
        .map(|b| {
            let is_ephemeral = b.lifecycle == "ephemeral";
            let status_color = if is_ephemeral {
                t.orphaned
            } else {
                match b.status {
                    ContainerStatus::Running => t.running,
                    ContainerStatus::Stopped => t.stopped,
                }
            };
            let status_str = if is_ephemeral {
                "⚠ orphaned"
            } else {
                match b.status {
                    ContainerStatus::Running => "● running",
                    ContainerStatus::Stopped => "○ stopped",
                }
            };
            let agent_label = match &b.project_name {
                Some(pn) if !pn.is_empty() => format!("{} — {}", b.agent_display_name, pn),
                _ => b.agent_display_name.clone(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<24}", b.box_name),
                    Style::default().fg(t.text).bold(),
                ),
                Span::styled(
                    format!("{:<30}", agent_label),
                    Style::default().fg(t.text_dim),
                ),
                Span::styled(status_str, Style::default().fg(status_color)),
            ]))
        })
        .collect();

    items.push(ListItem::new(Line::from(vec![Span::styled(
        "  ＋ New box",
        Style::default().fg(t.new_item).bold(),
    )])));

    let has_ephemeral = app.boxes.iter().any(|b| b.lifecycle == "ephemeral");
    let title = if app.boxes.is_empty() {
        " No boxes — press n to create one "
    } else if has_ephemeral {
        " Boxes  (⚠ orphaned — Enter → Kill to clean up) "
    } else {
        " Boxes "
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().fg(t.selection).bold());

    let mut state = app.boxes_list_state.clone();
    state.select(Some(app.boxes_idx.min(total - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_box_detail(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let info_height = if app.detail_stats.is_some() { 8 } else { 7 };
    let chunks =
        Layout::vertical([Constraint::Length(info_height), Constraint::Min(0)]).split(area);

    let info = if let Some(b) = &app.detail_box {
        let status_str = if b.lifecycle == "ephemeral" {
            "orphaned (not running)".to_string()
        } else {
            match b.status {
                ContainerStatus::Running => "running".to_string(),
                ContainerStatus::Stopped => "stopped".to_string(),
            }
        };
        let folder = b.folder.as_deref().unwrap_or("—");
        let mut lines = format!(
            "\n  Name:      {}\n  Agent:     {}\n  Lifecycle: {}\n  Status:    {}\n  Folder:    {}",
            b.box_name, b.agent_display_name, b.lifecycle, status_str, folder
        );
        if let Some(s) = &app.detail_stats {
            let mem_str = if s.mem_limit_mb > 0.0 {
                format!("{:.0} / {:.0} MiB", s.mem_mb, s.mem_limit_mb)
            } else {
                format!("{:.0} MiB", s.mem_mb)
            };
            lines.push_str(&format!(
                "\n  Resources: CPU {:.1}%  MEM {}",
                s.cpu_pct, mem_str
            ));
        }
        lines
    } else {
        String::new()
    };
    let info_widget = Paragraph::new(info).block(
        Block::default()
            .title(" Box info ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border_header)),
    );
    frame.render_widget(info_widget, chunks[0]);

    let action_items: Vec<ListItem> = app
        .detail_actions()
        .iter()
        .map(|(_, label)| {
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(*label, Style::default().fg(t.text)),
            ]))
        })
        .collect();
    let list = List::new(action_items)
        .block(
            Block::default()
                .title(" Actions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().fg(t.selection).bold());
    let mut state = app.detail_list_state.clone();
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn render_images(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    if app.cache_images.is_empty() {
        let widget = Paragraph::new(
            "\n  No cache images found.\n\n  Cache images are created the first time each agent is launched.\n  They speed up subsequent launches by skipping reinstallation.",
        )
        .style(Style::default().fg(t.text_dim))
        .block(
            Block::default()
                .title(" Cache images ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border)),
        );
        frame.render_widget(widget, area);
        return;
    }

    let items: Vec<ListItem> = app
        .cache_images
        .iter()
        .map(|img| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<20}", img.agent_id),
                    Style::default().fg(t.text).bold(),
                ),
                Span::styled(
                    format!("{:<44}", img.image_name),
                    Style::default().fg(t.text_dim),
                ),
                Span::styled(
                    format!("{:.1} MB", img.size_mb),
                    Style::default().fg(t.accent),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " Cache images ({}) — d/Del to remove ",
                    app.cache_images.len()
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().fg(t.selection).bold());

    let mut state = app.images_list_state.clone();
    state.select(if app.cache_images.is_empty() {
        None
    } else {
        Some(app.images_idx)
    });
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_agent(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|a| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<24}", a.display_name),
                    Style::default().fg(t.text).bold(),
                ),
                Span::styled(format!("({})", a.id), Style::default().fg(t.text_dim)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Select agent ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.border)),
        )
        .highlight_symbol("▶ ")
        .highlight_style(Style::default().fg(t.selection).bold());

    let mut state = app.agent_list_state.clone();
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_folder(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let chunks = Layout::vertical([
        Constraint::Length(3), // folder path
        Constraint::Length(1), // error
        Constraint::Length(3), // project name
        Constraint::Min(0),
    ])
    .split(area);
    frame.render_widget(
        app.folder.widget("Folder path", app.folder_focus == 0, t),
        chunks[0],
    );
    if let Some(err) = &app.folder_err {
        frame.render_widget(
            Paragraph::new(err.as_str()).style(Style::default().fg(t.error)),
            chunks[1],
        );
    }
    frame.render_widget(
        app.project_name.widget(
            "Project name  (optional — shown in window title and box list)",
            app.folder_focus == 1,
            t,
        ),
        chunks[2],
    );
}

fn render_lifecycle(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let daemon_locked = app.selected_agent_is_daemon();
    let show_name = app.is_persistent();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        if show_name {
            Constraint::Length(3)
        } else {
            Constraint::Length(0)
        },
        Constraint::Min(0),
    ])
    .split(area);

    let labels = ["ephemeral", "persistent"];
    let (left, right) = if daemon_locked {
        ("  ", "  ")
    } else {
        (
            if app.lifecycle_idx > 0 { "◀ " } else { "  " },
            if app.lifecycle_idx < 1 { " ▶" } else { "  " },
        )
    };
    let focused_type = app.prov_focus == 0;
    let border_style = if focused_type {
        Style::default().fg(t.border_focused)
    } else {
        Style::default().fg(t.border)
    };
    let desc = if daemon_locked {
        "Locked — daemon agents always run as persistent named boxes"
    } else if app.is_persistent() {
        "Named box — survives sessions, retains history and credentials"
    } else {
        "Fresh container every run, removed on exit"
    };
    let title = if daemon_locked {
        "Lifecycle  (locked)"
    } else {
        "Lifecycle  (← →)"
    };
    let type_text = Line::from(vec![
        Span::styled(left, Style::default().fg(t.text_dim)),
        Span::styled(
            labels[app.lifecycle_idx],
            Style::default().fg(t.text).bold(),
        ),
        Span::styled(right, Style::default().fg(t.text_dim)),
        Span::styled(format!("  — {desc}"), Style::default().fg(t.text_dim)),
    ]);
    let type_widget = Paragraph::new(type_text).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    frame.render_widget(type_widget, chunks[0]);

    if show_name {
        frame.render_widget(
            app.box_name
                .widget("Box name  (used to reconnect)", app.prov_focus == 1, t),
            chunks[1],
        );
    }
}

fn render_provider(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .split(area);

    let left = if app.prov_type_idx > 0 { "◀ " } else { "  " };
    let right = if app.prov_type_idx + 1 < PROVIDER_TYPES.len() {
        " ▶"
    } else {
        "  "
    };
    let focused = app.prov_focus == 0;
    let border_style = if focused {
        Style::default().fg(t.border_focused)
    } else {
        Style::default().fg(t.border)
    };
    let type_text = Line::from(vec![
        Span::styled(left, Style::default().fg(t.text_dim)),
        Span::styled(
            pt_label(app.current_provider_type()),
            Style::default().fg(t.text).bold(),
        ),
        Span::styled(right, Style::default().fg(t.text_dim)),
    ]);
    let type_widget = Paragraph::new(type_text)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .title("Provider type  (← →)")
                .borders(Borders::ALL)
                .border_style(border_style),
        );
    frame.render_widget(type_widget, chunks[0]);

    let prov_name_title = if app.is_pi() {
        "Provider name  (must match key in models.json, e.g. 'ollama')"
    } else {
        "Provider name"
    };
    frame.render_widget(
        app.prov_name
            .widget(prov_name_title, app.prov_focus == 1, t),
        chunks[2],
    );
    frame.render_widget(
        app.prov_model.widget("Model", app.prov_focus == 2, t),
        chunks[3],
    );

    let compat = *app.current_provider_type() == ProviderType::OpenaiCompatible;
    let url_title = if compat {
        "Base URL"
    } else {
        "Base URL  (not needed for this provider)"
    };
    let url_style = if compat {
        Style::default()
    } else {
        Style::default().fg(t.text_dim)
    };
    frame.render_widget(
        app.prov_base_url
            .widget(url_title, app.prov_focus == 3, t)
            .style(url_style),
        chunks[4],
    );

    frame.render_widget(
        app.prov_auth.widget(
            "Auth  (${env:…} / ${file:…} / ${keychain:…} / none / oauth)",
            app.prov_focus == 4,
            t,
        ),
        chunks[5],
    );
}

fn render_pi_models(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(if app.pi_models_err.is_some() { 1 } else { 0 }),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(
            "Pi models.json (JSON format). Leave empty to skip. F5 to continue.\n\
             Example: {\"providers\":{\"ollama\":{\"baseUrl\":\"http://host:11434/v1\",\"api\":\"openai-completions\",\"apiKey\":\"ollama\",\"models\":[{\"id\":\"llama3.1:8b\"}]}}}"
        ).style(Style::default().fg(t.text_dim)),
        chunks[0],
    );

    frame.render_widget(app.pi_models.widget("models.json", t), chunks[1]);

    if let Some(err) = &app.pi_models_err {
        frame.render_widget(
            Paragraph::new(err.as_str()).style(Style::default().fg(t.error)),
            chunks[2],
        );
    }
}

fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
    let t = app.theme();
    let agent = &app.agents[app.agent_idx];
    let pt = app.current_provider_type();
    let lifecycle = if app.is_persistent() {
        format!("persistent  (name: {})", app.box_name.value())
    } else {
        "ephemeral".to_string()
    };
    let base_url_line = if *pt == ProviderType::OpenaiCompatible {
        format!("  Base URL:      {}\n", app.prov_base_url.value())
    } else {
        String::new()
    };
    let prov_name = if app.prov_name.value().is_empty() {
        pt_label(pt).to_string()
    } else {
        app.prov_name.value()
    };
    let project_line = {
        let pn = app.project_name.value();
        if pn.trim().is_empty() {
            String::new()
        } else {
            format!("  Project:       {pn}\n")
        }
    };

    let text = format!(
        "\n  Agent:         {} ({})\n  Folder:        {}\n{project_line}  Lifecycle:     {}\n\n  Provider type: {}\n  Provider name: {}\n  Model:         {}\n{}  Auth:          {}\n\n  Press Enter to launch.",
        agent.display_name,
        agent.id,
        app.folder.value(),
        lifecycle,
        pt_label(pt),
        prov_name,
        app.prov_model.value(),
        base_url_line,
        app.prov_auth.value(),
    );

    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .title(" Ready to launch ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.success)),
        ),
        area,
    );
}

fn find_manifests_dir() -> Option<PathBuf> {
    // Check cwd first (normal dev workflow)
    if let Ok(cwd) = std::env::current_dir() {
        let d = cwd.join("manifests");
        if d.is_dir() {
            return Some(d);
        }
    }
    // Walk up from executable (handles GUI-launched terminals)
    let mut dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    while let Some(d) = dir {
        let candidate = d.join("manifests");
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}
