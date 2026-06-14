use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use agentbox_core::config::{
    AgentId, BoxConfig, FolderConfig, Lifecycle, NetworkMode, ProviderConfig, ProviderType,
    ResourceConfig, SyncMode,
};
use agentbox_core::manifest;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
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
}

/// Entry point: runs the wizard synchronously. Call from `spawn_blocking`.
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

// ── State machine ─────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Agent,
    Folder,
    Provider,
    Summary,
}

#[derive(Clone)]
struct AgentEntry {
    id: String,
    display_name: String,
    source: &'static str, // "manifest" or "built-in"
}

/// Simple text-input field with a character-level cursor.
struct Input {
    chars: Vec<char>,
    cursor: usize,
}

impl Input {
    fn new(initial: &str) -> Self {
        let chars: Vec<char> = initial.chars().collect();
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
            let (cursor_ch, after): (String, String) = if self.cursor < self.chars.len() {
                (
                    self.chars[self.cursor].to_string(),
                    self.chars[self.cursor + 1..].iter().collect(),
                )
            } else {
                (" ".to_string(), String::new())
            };
            Text::from(Line::from(vec![
                Span::raw(before),
                Span::styled(cursor_ch, Style::default().bg(Color::Yellow).fg(Color::Black)),
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

fn provider_type_label(pt: &ProviderType) -> &'static str {
    match pt {
        ProviderType::Anthropic => "anthropic",
        ProviderType::Openai => "openai",
        ProviderType::OpenaiCompatible => "openai-compatible",
    }
}

fn default_auth_for(pt: &ProviderType) -> &'static str {
    match pt {
        ProviderType::Anthropic => "${env:ANTHROPIC_API_KEY}",
        ProviderType::Openai => "${env:OPENAI_API_KEY}",
        ProviderType::OpenaiCompatible => "${env:OPENAI_API_KEY}",
    }
}

struct App {
    screen: Screen,

    // Agent screen
    agents: Vec<AgentEntry>,
    agent_idx: usize,
    agent_list_state: ListState,

    // Folder screen
    folder: Input,
    folder_err: Option<String>,

    // Provider screen — Tab cycles through fields 0-4
    prov_type_idx: usize,
    prov_name: Input,
    prov_model: Input,
    prov_base_url: Input,
    prov_auth: Input,
    prov_focus: usize, // 0=type, 1=name, 2=model, 3=base_url, 4=auth

    // Manifests dir used for both discovery and later engine call
    manifests_dir: Option<PathBuf>,
}

impl App {
    fn new() -> Self {
        let manifests_dir = std::env::current_dir()
            .ok()
            .map(|d| d.join("manifests"))
            .filter(|d| d.is_dir());

        // Merge manifest agents + built-ins (manifests take precedence).
        let manifest_agents: Vec<AgentEntry> = manifests_dir
            .as_deref()
            .map(manifest::list_manifests)
            .unwrap_or_default()
            .into_iter()
            .map(|(id, display_name)| AgentEntry {
                id,
                display_name,
                source: "manifest",
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
                source: "built-in",
            })
            .collect();

        let agents: Vec<AgentEntry> = manifest_agents.into_iter().chain(builtins).collect();

        let mut agent_list_state = ListState::default();
        agent_list_state.select(Some(0));

        App {
            screen: Screen::Agent,
            agents,
            agent_idx: 0,
            agent_list_state,

            folder: Input::new(""),
            folder_err: None,

            prov_type_idx: 0,
            prov_name: Input::new(""),
            prov_model: Input::new(""),
            prov_base_url: Input::new(""),
            prov_auth: Input::new(default_auth_for(&PROVIDER_TYPES[0])),
            prov_focus: 0,

            manifests_dir,
        }
    }

    fn current_provider_type(&self) -> &ProviderType {
        &PROVIDER_TYPES[self.prov_type_idx]
    }

    // Number of focusable fields on the provider screen.
    fn max_prov_focus(&self) -> usize {
        4 // 0=type, 1=name, 2=model, 3=base_url, 4=auth — always 5 fields
    }

    fn build_config(&self) -> BoxConfig {
        let pt = self.current_provider_type().clone();
        let base_url = if pt == ProviderType::OpenaiCompatible {
            let s = self.prov_base_url.value();
            if s.is_empty() { None } else { Some(s) }
        } else {
            None
        };

        BoxConfig {
            agent: AgentId(self.agents[self.agent_idx].id.clone()),
            folder: FolderConfig {
                path: PathBuf::from(self.folder.value()),
                sync: SyncMode::Mount,
            },
            lifecycle: Lifecycle::Ephemeral,
            provider: ProviderConfig {
                name: if self.prov_name.value().is_empty() {
                    provider_type_label(&pt).to_string()
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
}

// ── Event loop ────────────────────────────────────────────────────────────────

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<WizardResult> {
    loop {
        terminal.draw(|f| render(f, app))?;

        if let Event::Key(key) = event::read()? {
            // Ctrl-C / q on any screen → quit
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
            }
        }
    }
}

enum Action {
    Continue,
    Quit,
    Launch,
}

fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    match app.screen {
        Screen::Agent => handle_agent(app, key),
        Screen::Folder => handle_folder(app, key),
        Screen::Provider => handle_provider(app, key),
        Screen::Summary => handle_summary(app, key),
    }
}

fn handle_agent(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
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
            app.screen = Screen::Folder;
        }
        _ => {}
    }
    Action::Continue
}

fn handle_folder(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.folder_err = None;
            app.screen = Screen::Agent;
        }
        KeyCode::Enter => {
            let path = PathBuf::from(app.folder.value());
            if !path.exists() {
                app.folder_err = Some(format!("Path does not exist: {}", path.display()));
            } else if !path.is_dir() {
                app.folder_err = Some("Path is not a directory".into());
            } else {
                app.folder_err = None;
                app.screen = Screen::Provider;
            }
        }
        _ => {
            app.folder.on_key(&key);
            app.folder_err = None;
        }
    }
    Action::Continue
}

fn handle_provider(app: &mut App, key: KeyEvent) -> Action {
    let focus = app.prov_focus;
    match key.code {
        KeyCode::Esc => {
            app.prov_focus = 0;
            app.screen = Screen::Folder;
        }
        KeyCode::Tab => {
            app.prov_focus = (app.prov_focus + 1) % (app.max_prov_focus() + 1);
        }
        KeyCode::BackTab => {
            if app.prov_focus == 0 {
                app.prov_focus = app.max_prov_focus();
            } else {
                app.prov_focus -= 1;
            }
        }
        KeyCode::Enter if focus == app.max_prov_focus() => {
            app.screen = Screen::Summary;
        }
        KeyCode::Enter => {
            app.prov_focus = (focus + 1) % (app.max_prov_focus() + 1);
        }
        // Provider type: left/right when focused on field 0
        KeyCode::Left if focus == 0 => {
            if app.prov_type_idx > 0 {
                app.prov_type_idx -= 1;
                app.prov_auth = Input::new(default_auth_for(app.current_provider_type()));
            }
        }
        KeyCode::Right if focus == 0 => {
            if app.prov_type_idx + 1 < PROVIDER_TYPES.len() {
                app.prov_type_idx += 1;
                app.prov_auth = Input::new(default_auth_for(app.current_provider_type()));
            }
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

fn handle_summary(app: &mut App, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::Provider;
        }
        KeyCode::Enter => return Action::Launch,
        KeyCode::Char('q') => return Action::Quit,
        _ => {}
    }
    Action::Continue
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Global layout: header (3) | body | footer (3)
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);

    render_header(frame, chunks[0], app);
    match app.screen {
        Screen::Agent => render_agent(frame, chunks[1], app),
        Screen::Folder => render_folder(frame, chunks[1], app),
        Screen::Provider => render_provider(frame, chunks[1], app),
        Screen::Summary => render_summary(frame, chunks[1], app),
    }
    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.screen {
        Screen::Agent => " agentbox — select agent (1/4) ",
        Screen::Folder => " agentbox — workspace folder (2/4) ",
        Screen::Provider => " agentbox — provider config (3/4) ",
        Screen::Summary => " agentbox — ready to launch (4/4) ",
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
        Screen::Agent => "↑↓/jk navigate  Enter select  q quit",
        Screen::Folder => "Type path  Enter confirm  Esc back",
        Screen::Provider => "Tab/↵ next field  ←→ type (field 1)  Esc back",
        Screen::Summary => "Enter launch  Esc back  q quit",
    };
    let p = Paragraph::new(help)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(p, area);
}

fn render_agent(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|a| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<22}", a.display_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("({})  ", a.id),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(a.source, Style::default().fg(Color::DarkGray).italic()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)))
        .highlight_symbol("► ")
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = app.agent_list_state.clone();
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_folder(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);

    frame.render_widget(app.folder.widget("Folder path", true), chunks[0]);

    if let Some(err) = &app.folder_err {
        let p = Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red));
        frame.render_widget(p, chunks[1]);
    }
}

fn render_provider(frame: &mut Frame, area: Rect, app: &App) {
    // 5 fields + gaps
    let chunks = Layout::vertical([
        Constraint::Length(3), // provider type selector
        Constraint::Length(1), // spacer
        Constraint::Length(3), // name
        Constraint::Length(3), // model
        Constraint::Length(3), // base_url
        Constraint::Length(3), // auth
        Constraint::Min(0),
    ])
    .split(area);

    // Field 0: provider type (not a text input — rendered as a selector row)
    let pt_label = provider_type_label(app.current_provider_type());
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
        Span::styled(pt_label, Style::default().fg(Color::White).bold()),
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
    // spacer at chunks[1] — nothing to render

    frame.render_widget(app.prov_name.widget("Provider name", app.prov_focus == 1), chunks[2]);
    frame.render_widget(app.prov_model.widget("Model", app.prov_focus == 2), chunks[3]);

    // Base URL: dim when not openai-compatible
    let base_url_style = if *app.current_provider_type() != ProviderType::OpenaiCompatible {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    let base_url_title = if *app.current_provider_type() != ProviderType::OpenaiCompatible {
        "Base URL  (not used for this provider type)"
    } else {
        "Base URL"
    };
    let base_url_widget = app
        .prov_base_url
        .widget(base_url_title, app.prov_focus == 3)
        .style(base_url_style);
    frame.render_widget(base_url_widget, chunks[4]);

    frame.render_widget(
        app.prov_auth
            .widget("Auth  (${env:…} / ${file:…} / ${keychain:…} / none)", app.prov_focus == 4),
        chunks[5],
    );
}

fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
    let agent = &app.agents[app.agent_idx];
    let pt = app.current_provider_type();
    let base_url_line = if *pt == ProviderType::OpenaiCompatible {
        format!("  Base URL:      {}\n", app.prov_base_url.value())
    } else {
        String::new()
    };
    let prov_name = if app.prov_name.value().is_empty() {
        provider_type_label(pt).to_string()
    } else {
        app.prov_name.value()
    };

    let text = format!(
        "\n  Agent:         {} ({})\n  Folder:        {}\n\n  Provider type: {}\n  Provider name: {}\n  Model:         {}\n{}  Auth:          {}\n\n  Press Enter to launch.",
        agent.display_name,
        agent.id,
        app.folder.value(),
        provider_type_label(pt),
        prov_name,
        app.prov_model.value(),
        base_url_line,
        app.prov_auth.value(),
    );

    let p = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        );
    frame.render_widget(p, area);
}
