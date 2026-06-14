use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

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
    Home,         // list existing boxes + "New box"
    BoxDetail,    // manage a selected persistent box
    WizardAgent,
    WizardFolder,
    WizardLifecycle,
    WizardProvider,
    WizardSummary,
}

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AgentEntry {
    id: String,
    display_name: String,
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
    fn widget<'a>(&self, title: &'a str, focused: bool) -> Paragraph<'a> {
        let border_style = if focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
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
                Span::styled(cur, Style::default().bg(Color::Yellow).fg(Color::Black)),
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
    Remove,
}

const BOX_ACTIONS: [(BoxAction, &str); 3] = [
    (BoxAction::Attach, "Attach  (reconnect and launch agent)"),
    (BoxAction::Stop, "Stop    (halt container, keep state)"),
    (BoxAction::Remove, "Remove  (delete container + state volume)"),
];

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

    // Wizard — agent
    agents: Vec<AgentEntry>,
    agent_idx: usize,
    agent_list_state: ListState,

    // Wizard — folder
    folder: Input,
    folder_err: Option<String>,

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

    // Status / error
    status_msg: Option<(String, bool)>, // (message, is_error)

    manifests_dir: Option<PathBuf>,
}

impl App {
    fn new() -> Self {
        let manifests_dir = std::env::current_dir()
            .ok()
            .map(|d| d.join("manifests"))
            .filter(|d| d.is_dir());

        // Load existing boxes
        let boxes = do_async(agentbox_core::list_boxes()).unwrap_or_default();
        let mut boxes_list_state = ListState::default();
        if !boxes.is_empty() {
            boxes_list_state.select(Some(0));
        }

        // Gather available agents
        let manifest_agents: Vec<AgentEntry> = manifests_dir
            .as_deref()
            .map(manifest::list_manifests)
            .unwrap_or_default()
            .into_iter()
            .map(|(id, display_name)| AgentEntry { id, display_name })
            .collect();
        let manifest_ids: std::collections::HashSet<&str> =
            manifest_agents.iter().map(|a| a.id.as_str()).collect();
        let builtins: Vec<AgentEntry> = [("claude-code", "Claude Code"), ("opencode", "OpenCode")]
            .iter()
            .filter(|(id, _)| !manifest_ids.contains(*id))
            .map(|(id, name)| AgentEntry {
                id: id.to_string(),
                display_name: name.to_string(),
            })
            .collect();
        let agents: Vec<AgentEntry> = manifest_agents.into_iter().chain(builtins).collect();

        let mut agent_list_state = ListState::default();
        agent_list_state.select(Some(0));

        let mut detail_list_state = ListState::default();
        detail_list_state.select(Some(0));

        App {
            screen: Screen::Home,
            boxes,
            boxes_idx: 0,
            boxes_list_state,

            detail_box: None,
            detail_action_idx: 0,
            detail_list_state,

            agents,
            agent_idx: 0,
            agent_list_state,

            folder: Input::new(""),
            folder_err: None,

            lifecycle_idx: 0,
            box_name: Input::new(""),

            prov_type_idx: 0,
            prov_name: Input::new(""),
            prov_model: Input::new(""),
            prov_base_url: Input::new(""),
            prov_auth: Input::new(default_auth(&PROVIDER_TYPES[0])),
            prov_focus: 0,

            status_msg: None,
            manifests_dir,
        }
    }

    fn current_provider_type(&self) -> &ProviderType {
        &PROVIDER_TYPES[self.prov_type_idx]
    }

    fn is_persistent(&self) -> bool {
        self.lifecycle_idx == 1
    }

    fn build_config(&self) -> BoxConfig {
        let pt = self.current_provider_type().clone();
        let base_url = if pt == ProviderType::OpenaiCompatible {
            let s = self.prov_base_url.value();
            if s.is_empty() { None } else { Some(s) }
        } else {
            None
        };
        let name = if self.is_persistent() {
            let n = self.box_name.value();
            if n.is_empty() { None } else { Some(n) }
        } else {
            None
        };
        BoxConfig {
            agent: AgentId(self.agents[self.agent_idx].id.clone()),
            name,
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
                raw: serde_json::Value::Null,
            },
            network: NetworkMode::Open,
            resources: ResourceConfig { cpus: None, memory: None },
            extra_env: HashMap::new(),
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
        terminal.draw(|f| render(f, app))?;

        if let Event::Key(key) = event::read()? {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(WizardResult::Cancelled);
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
        Screen::WizardAgent => handle_agent(app, key),
        Screen::WizardFolder => handle_folder(app, key),
        Screen::WizardLifecycle => handle_lifecycle(app, key),
        Screen::WizardProvider => handle_provider(app, key),
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
        _ => {}
    }
    Action::Continue
}

// ── Box detail ────────────────────────────────────────────────────────────────

fn handle_box_detail(app: &mut App, key: KeyEvent) -> Action {
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
            if app.detail_action_idx + 1 < BOX_ACTIONS.len() {
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
            let action = BOX_ACTIONS[app.detail_action_idx].0;
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
            app.screen = Screen::WizardAgent;
        }
        KeyCode::Enter => {
            let path = PathBuf::from(app.folder.value());
            if !path.exists() {
                app.folder_err = Some(format!("Path does not exist: {}", path.display()));
            } else if !path.is_dir() {
                app.folder_err = Some("Path is not a directory".into());
            } else {
                app.folder_err = None;
                app.screen = Screen::WizardLifecycle;
            }
        }
        _ => {
            app.folder.on_key(&key);
            app.folder_err = None;
        }
    }
    Action::Continue
}

// ── Wizard: lifecycle ─────────────────────────────────────────────────────────

fn handle_lifecycle(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::WizardFolder;
        }
        KeyCode::Left if app.lifecycle_idx > 0 && app.prov_focus == 0 => {
            app.lifecycle_idx -= 1;
        }
        KeyCode::Right if app.lifecycle_idx < 1 && app.prov_focus == 0 => {
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
            app.prov_focus = if app.prov_focus == 0 { 4 } else { app.prov_focus - 1 };
        }
        KeyCode::Enter if focus == 4 => {
            app.prov_focus = 0;
            app.screen = Screen::WizardSummary;
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
        Screen::WizardAgent => render_agent(frame, chunks[1], app),
        Screen::WizardFolder => render_folder(frame, chunks[1], app),
        Screen::WizardLifecycle => render_lifecycle(frame, chunks[1], app),
        Screen::WizardProvider => render_provider(frame, chunks[1], app),
        Screen::WizardSummary => render_summary(frame, chunks[1], app),
    }
    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.screen {
        Screen::Home => " agentbox ",
        Screen::BoxDetail => " agentbox — manage box ",
        Screen::WizardAgent => " agentbox — new box (1/5) select agent ",
        Screen::WizardFolder => " agentbox — new box (2/5) workspace folder ",
        Screen::WizardLifecycle => " agentbox — new box (3/5) lifecycle ",
        Screen::WizardProvider => " agentbox — new box (4/5) provider ",
        Screen::WizardSummary => " agentbox — new box (5/5) ready to launch ",
    };
    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let help = match app.screen {
        Screen::Home => "↑↓ navigate  Enter open  n new box  r refresh  q quit",
        Screen::BoxDetail => "↑↓ select action  Enter execute  Esc back",
        Screen::WizardAgent => "↑↓/jk navigate  Enter select  Esc back",
        Screen::WizardFolder => "Type path  Enter next  Esc back",
        Screen::WizardLifecycle => "← → type  Tab name field  Enter next  Esc back",
        Screen::WizardProvider => "Tab/↵ next field  ← → type (field 1)  Esc back",
        Screen::WizardSummary => "Enter launch  Esc back  q quit",
    };

    let content = if let Some((msg, is_err)) = &app.status_msg {
        let style = if *is_err {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        Paragraph::new(msg.as_str())
            .style(style)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL))
    } else {
        Paragraph::new(help)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL))
    };

    frame.render_widget(content, area);
}

fn render_home(frame: &mut Frame, area: Rect, app: &App) {
    let total = app.boxes.len() + 1;
    let mut items: Vec<ListItem> = app
        .boxes
        .iter()
        .map(|b| {
            let status_color = match b.status {
                ContainerStatus::Running => Color::Green,
                ContainerStatus::Stopped => Color::DarkGray,
            };
            let status_str = match b.status {
                ContainerStatus::Running => "● running",
                ContainerStatus::Stopped => "○ stopped",
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<22}", b.box_name),
                    Style::default().fg(Color::White).bold(),
                ),
                Span::styled(
                    format!("{:<20} ", b.agent_display_name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(status_str, Style::default().fg(status_color)),
            ]))
        })
        .collect();

    // "New box" entry
    items.push(ListItem::new(Line::from(vec![Span::styled(
        "  + New box",
        Style::default().fg(Color::Cyan),
    )])));

    let list = List::new(items)
        .block(
            Block::default()
                .title(if app.boxes.is_empty() {
                    " No persistent boxes — press Enter or 'n' to create one "
                } else {
                    " Persistent boxes "
                })
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_symbol("► ")
        .highlight_style(Style::default().fg(Color::Yellow).bold());

    let mut state = app.boxes_list_state.clone();
    // Ensure selection reflects current index (including "New box")
    state.select(Some(app.boxes_idx.min(total - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_box_detail(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).split(area);

    // Info panel
    let info = if let Some(b) = &app.detail_box {
        let status_str = match b.status {
            ContainerStatus::Running => "running",
            ContainerStatus::Stopped => "stopped",
        };
        let folder = b.folder.as_deref().unwrap_or("—");
        format!(
            "\n  Name:    {}\n  Agent:   {}\n  Status:  {}\n  Folder:  {}",
            b.box_name, b.agent_display_name, status_str, folder
        )
    } else {
        String::new()
    };
    let info_widget = Paragraph::new(info).block(
        Block::default()
            .title(" Box info ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(info_widget, chunks[0]);

    // Action list
    let action_items: Vec<ListItem> = BOX_ACTIONS
        .iter()
        .map(|(_, label)| ListItem::new(format!("  {}", label)))
        .collect();
    let list = List::new(action_items)
        .block(
            Block::default()
                .title(" Actions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_symbol("► ")
        .highlight_style(Style::default().fg(Color::Yellow).bold());
    let mut state = app.detail_list_state.clone();
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn render_agent(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|a| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<24}", a.display_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("({})", a.id),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_symbol("► ")
        .highlight_style(Style::default().fg(Color::Yellow).bold());

    let mut state = app.agent_list_state.clone();
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_folder(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    frame.render_widget(app.folder.widget("Folder path", true), chunks[0]);
    if let Some(err) = &app.folder_err {
        frame.render_widget(
            Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red)),
            chunks[1],
        );
    }
}

fn render_lifecycle(frame: &mut Frame, area: Rect, app: &App) {
    let show_name = app.is_persistent();
    let chunks = Layout::vertical([
        Constraint::Length(3), // lifecycle selector
        if show_name { Constraint::Length(3) } else { Constraint::Length(0) }, // box name
        Constraint::Min(0),
    ])
    .split(area);

    // Lifecycle type selector (same style as provider type selector)
    let labels = ["ephemeral", "persistent"];
    let left = if app.lifecycle_idx > 0 { "◄ " } else { "  " };
    let right = if app.lifecycle_idx < 1 { " ►" } else { "  " };
    let focused_type = app.prov_focus == 0;
    let border_style = if focused_type {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let desc = if app.is_persistent() {
        "Named box — survives sessions, retains history and credentials"
    } else {
        "Fresh container every run, removed on exit"
    };
    let type_text = Line::from(vec![
        Span::styled(left, Style::default().fg(Color::DarkGray)),
        Span::styled(labels[app.lifecycle_idx], Style::default().fg(Color::White).bold()),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
        Span::styled(format!("  — {desc}"), Style::default().fg(Color::DarkGray)),
    ]);
    let type_widget = Paragraph::new(type_text).block(
        Block::default()
            .title("Lifecycle  (← →)")
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    frame.render_widget(type_widget, chunks[0]);

    if show_name {
        frame.render_widget(
            app.box_name.widget("Box name  (used to reconnect)", app.prov_focus == 1),
            chunks[1],
        );
    }
}

fn render_provider(frame: &mut Frame, area: Rect, app: &App) {
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

    // Provider type selector
    let left = if app.prov_type_idx > 0 { "◄ " } else { "  " };
    let right = if app.prov_type_idx + 1 < PROVIDER_TYPES.len() { " ►" } else { "  " };
    let focused = app.prov_focus == 0;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let type_text = Line::from(vec![
        Span::styled(left, Style::default().fg(Color::DarkGray)),
        Span::styled(
            pt_label(app.current_provider_type()),
            Style::default().fg(Color::White).bold(),
        ),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
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

    frame.render_widget(app.prov_name.widget("Provider name", app.prov_focus == 1), chunks[2]);
    frame.render_widget(app.prov_model.widget("Model", app.prov_focus == 2), chunks[3]);

    let compat = *app.current_provider_type() == ProviderType::OpenaiCompatible;
    let url_title = if compat { "Base URL" } else { "Base URL  (not needed)" };
    let url_style = if compat { Style::default() } else { Style::default().fg(Color::DarkGray) };
    frame.render_widget(
        app.prov_base_url
            .widget(url_title, app.prov_focus == 3)
            .style(url_style),
        chunks[4],
    );

    frame.render_widget(
        app.prov_auth
            .widget("Auth  (${env:…} / ${file:…} / ${keychain:…} / none)", app.prov_focus == 4),
        chunks[5],
    );
}

fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
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

    let text = format!(
        "\n  Agent:         {} ({})\n  Folder:        {}\n  Lifecycle:     {}\n\n  Provider type: {}\n  Provider name: {}\n  Model:         {}\n{}  Auth:          {}\n\n  Press Enter to launch.",
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
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        ),
        area,
    );
}
