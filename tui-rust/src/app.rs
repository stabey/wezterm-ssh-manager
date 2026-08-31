use std::{
    io::{self, Stdout},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    model::{
        ALL_GROUPS, draft_from_profile, group_summaries, profile_target, raw_from_draft,
        visible_profiles,
    },
    protocol::RequestProtocol,
    sftp::{
        self, CompatibilityIssueSeverity, ConnectionRole, CredentialOverrides, FileEntry, FileKind,
        FileProvider, LocalFileProvider, MkdirOptions, OperationOptions,
        ProfileConnectionOverrides, RemoteFileProvider, RemoveOptions, SftpError, SftpSession,
        TransferDirection, TransferOptions, TransferProgress,
    },
    snapshot::read_snapshot,
    types::{MainPage, ManagerFocus, ManagerRequest, Profile, ProfileDraft, Snapshot},
    ui,
};

const FORM_FIELDS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SftpSide {
    #[default]
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Directory,
    File,
    Link,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct SftpEntryView {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SftpPanelView {
    pub path: String,
    pub entries: Vec<SftpEntryView>,
    pub selected: usize,
    pub loading: bool,
    pub error: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TextInput {
    pub value: String,
    /// Cursor position in bytes. It is always kept on a UTF-8 grapheme boundary.
    pub cursor: usize,
    pub secret: bool,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self {
            value,
            cursor,
            secret: false,
        }
    }

    pub fn secret(value: impl Into<String>) -> Self {
        Self {
            secret: true,
            ..Self::new(value)
        }
    }

    fn previous_boundary(&self) -> usize {
        self.value[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self) -> usize {
        self.value[self.cursor..]
            .grapheme_indices(true)
            .nth(1)
            .map(|(index, _)| self.cursor + index)
            .unwrap_or(self.value.len())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.drain(..self.cursor);
                self.cursor = 0;
                true
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.truncate(self.cursor);
                true
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.value.insert(self.cursor, character);
                self.cursor += character.len_utf8();
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let previous = self.previous_boundary();
                    self.value.drain(previous..self.cursor);
                    self.cursor = previous;
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    let next = self.next_boundary();
                    self.value.drain(self.cursor..next);
                }
                true
            }
            KeyCode::Left => {
                self.cursor = self.previous_boundary();
                true
            }
            KeyCode::Right => {
                self.cursor = self.next_boundary();
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.cursor = self.value.len();
                true
            }
            _ => false,
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Modal {
    Filter(TextInput),
    Quick(TextInput),
    Edit {
        draft: ProfileDraft,
        field: usize,
        cursors: [usize; FORM_FIELDS],
    },
    Delete {
        profile: Profile,
    },
    SftpCredentials {
        profile: Profile,
        password: TextInput,
        jump_password: TextInput,
        field: usize,
    },
    SftpInput {
        mkdir: bool,
        side: SftpSide,
        input: TextInput,
        entry: Option<SftpEntryView>,
    },
    SftpDelete {
        side: SftpSide,
        entry: SftpEntryView,
    },
    SftpOverwrite {
        upload: bool,
        source: String,
        destination: String,
    },
}

impl Modal {
    pub fn title(&self) -> String {
        match self {
            Self::Filter(_) => "过滤".to_owned(),
            Self::Quick(_) => "快捷连接".to_owned(),
            Self::Edit { .. } => "编辑连接".to_owned(),
            Self::Delete { .. } => "确认删除".to_owned(),
            Self::SftpCredentials { profile, .. } => format!("SFTP 认证 · {}", profile.name),
            Self::SftpInput { mkdir: true, .. } => "新建目录".to_owned(),
            Self::SftpInput { mkdir: false, .. } => "改名".to_owned(),
            Self::SftpDelete { .. } => "确认删除文件".to_owned(),
            Self::SftpOverwrite { .. } => "确认覆盖".to_owned(),
        }
    }

    pub fn is_dangerous(&self) -> bool {
        matches!(
            self,
            Self::Delete { .. } | Self::SftpDelete { .. } | Self::SftpOverwrite { .. }
        )
    }
}

enum ConnectFailure {
    Credential(ConnectionRole),
    Message(String),
    Cancelled,
}

enum TransferOutcome {
    Completed,
    DestinationExists,
    Cancelled,
    Failed(String),
}

enum Mutation {
    Mkdir(String),
    Rename { from: String, to: String },
    Remove(String),
}

struct ConnectedEvent {
    sequence: u64,
    profile: Profile,
    fingerprint: String,
    warnings: Vec<String>,
    result: std::result::Result<(SftpSession, String), ConnectFailure>,
}

enum AsyncEvent {
    PanelLoaded {
        side: SftpSide,
        sequence: u64,
        path: String,
        result: std::result::Result<Vec<FileEntry>, String>,
    },
    Connected(Box<ConnectedEvent>),
    Disconnected {
        sequence: u64,
        detail: String,
    },
    TransferProgress(TransferProgress),
    TransferFinished {
        direction: TransferDirection,
        source: String,
        destination: String,
        outcome: TransferOutcome,
    },
    MutationFinished {
        side: SftpSide,
        success: String,
        result: std::result::Result<(), String>,
    },
}

pub(crate) struct App {
    pub snapshot_path: PathBuf,
    pub snapshot: Snapshot,
    pub protocol: RequestProtocol,
    pub page: MainPage,
    pub focus: ManagerFocus,
    pub group_index: usize,
    pub host_index: usize,
    pub filter: String,
    pub modal: Option<Modal>,
    pub status: String,
    pub sftp_side: SftpSide,
    pub local_panel: SftpPanelView,
    pub remote_panel: SftpPanelView,
    pub connected_profile: Option<Profile>,
    pub sftp_busy: bool,
    pub transfer_progress: Option<String>,
    pub terminal_width: u16,
    pub terminal_height: u16,
    event_sender: mpsc::UnboundedSender<AsyncEvent>,
    local_provider: LocalFileProvider,
    remote_provider: Option<RemoteFileProvider>,
    sftp_session: Option<Arc<Mutex<SftpSession>>>,
    connection_cancel: Option<CancellationToken>,
    operation_cancel: Option<CancellationToken>,
    connected_fingerprint: Option<String>,
    connection_sequence: u64,
    local_read_sequence: u64,
    remote_read_sequence: u64,
    should_quit: bool,
    last_snapshot_modified: Option<SystemTime>,
    last_host_click: Option<(usize, Instant)>,
}

impl App {
    fn new(
        snapshot_path: PathBuf,
        snapshot: Snapshot,
        protocol: RequestProtocol,
        event_sender: mpsc::UnboundedSender<AsyncEvent>,
    ) -> Self {
        let home = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .to_string_lossy()
            .into_owned();
        Self {
            snapshot_path,
            snapshot,
            protocol,
            page: MainPage::Manager,
            focus: ManagerFocus::Hosts,
            group_index: 0,
            host_index: 0,
            filter: String::new(),
            modal: None,
            status: "就绪".to_owned(),
            sftp_side: SftpSide::Local,
            local_panel: SftpPanelView {
                path: home,
                ..SftpPanelView::default()
            },
            remote_panel: SftpPanelView {
                path: "/".to_owned(),
                ..SftpPanelView::default()
            },
            connected_profile: None,
            sftp_busy: false,
            transfer_progress: None,
            terminal_width: 80,
            terminal_height: 24,
            event_sender,
            local_provider: LocalFileProvider,
            remote_provider: None,
            sftp_session: None,
            connection_cancel: None,
            operation_cancel: None,
            connected_fingerprint: None,
            connection_sequence: 0,
            local_read_sequence: 0,
            remote_read_sequence: 0,
            should_quit: false,
            last_snapshot_modified: None,
            last_host_click: None,
        }
    }

    pub fn groups(&self) -> Vec<crate::model::GroupSummary> {
        group_summaries(&self.snapshot)
    }

    pub fn selected_group(&self) -> String {
        self.groups()
            .get(self.group_index)
            .map(|group| group.id.clone())
            .unwrap_or_else(|| ALL_GROUPS.to_owned())
    }

    pub fn hosts(&self) -> Vec<&Profile> {
        visible_profiles(&self.snapshot, &self.selected_group(), &self.filter)
    }

    pub fn selected_host(&self) -> Option<&Profile> {
        self.hosts().get(self.host_index).copied()
    }

    pub fn show_details(&self) -> bool {
        self.terminal_width >= 102
    }

    fn clamp_selection(&mut self) {
        self.group_index = clamp_index(self.group_index, self.groups().len());
        self.host_index = clamp_index(self.host_index, self.hosts().len());
    }

    fn emit(&mut self, request: ManagerRequest, success: impl Into<String>) {
        match self.protocol.emit(&request) {
            Ok(_) => self.status = success.into(),
            Err(error) => self.status = error.to_string(),
        }
    }

    fn connect(&mut self, where_: String) {
        let Some(profile) = self.selected_host().cloned() else {
            self.status = "请先选择一台主机".to_owned();
            return;
        };
        let name = profile.name.clone();
        self.emit(
            ManagerRequest::Connect {
                id: profile.id,
                where_,
            },
            format!("正在连接 {name}…"),
        );
    }

    fn open_edit(&mut self, profile: Option<Profile>) {
        if let Some(profile) = profile.as_ref()
            && !profile.editable
        {
            self.emit(
                ManagerRequest::CopyIn {
                    id: profile.id.clone(),
                },
                "已请求复制到可编辑配置",
            );
            return;
        }
        let selected_group = self.selected_group();
        let initial_group = match selected_group.as_str() {
            ALL_GROUPS => "",
            group => group,
        };
        let draft = draft_from_profile(profile.as_ref(), initial_group);
        let cursors = std::array::from_fn(|index| draft_field(&draft, index).len());
        self.modal = Some(Modal::Edit {
            draft,
            field: 0,
            cursors,
        });
    }

    fn save_draft(&mut self) {
        let Some(Modal::Edit { draft, .. }) = self.modal.as_ref() else {
            return;
        };
        let result = raw_from_draft(draft);
        let Some(raw) = result.raw else {
            self.status = result.error.unwrap_or_else(|| "表单无效".to_owned());
            return;
        };
        let id = draft.original_id.clone();
        self.modal = None;
        self.emit(ManagerRequest::Upsert { id, raw }, "已请求保存");
    }

    fn move_group(&mut self, delta: isize) {
        self.group_index = move_index(self.group_index, delta, self.groups().len());
        self.host_index = 0;
    }

    fn move_host(&mut self, delta: isize) {
        self.host_index = move_index(self.host_index, delta, self.hosts().len());
    }

    fn handle_manager_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('2') | KeyCode::Char('s') => self.open_sftp_workspace(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    ManagerFocus::Groups => ManagerFocus::Hosts,
                    ManagerFocus::Hosts if self.show_details() => ManagerFocus::Details,
                    _ => ManagerFocus::Groups,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus == ManagerFocus::Groups {
                    self.move_group(-1);
                } else {
                    self.move_host(-1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.focus == ManagerFocus::Groups {
                    self.move_group(1);
                } else {
                    self.move_host(1);
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let where_ = if key.modifiers.contains(KeyModifiers::CONTROL) {
                    "window".to_owned()
                } else {
                    self.snapshot.default_where.clone()
                };
                self.connect(where_);
            }
            KeyCode::Char('/') => {
                self.modal = Some(Modal::Filter(TextInput::new(self.filter.clone())));
            }
            KeyCode::Char('p') => {
                self.modal = Some(Modal::Quick(TextInput::new("")));
            }
            KeyCode::Char('n') => self.open_edit(None),
            KeyCode::Char('e') => self.open_edit(self.selected_host().cloned()),
            KeyCode::Char('d') => {
                let Some(profile) = self.selected_host().cloned() else {
                    self.status = "请先选择一台主机".to_owned();
                    return;
                };
                if profile.editable {
                    self.modal = Some(Modal::Delete { profile });
                } else {
                    self.status = "只读连接不能删除".to_owned();
                }
            }
            KeyCode::Char('r') => self.emit(ManagerRequest::Reload, "已请求刷新"),
            KeyCode::Char('q') | KeyCode::Esc => {
                self.emit(ManagerRequest::Hide, "正在返回…");
            }
            _ => {}
        }
    }

    fn handle_sftp_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('1') => self.page = MainPage::Manager,
            KeyCode::Tab => {
                self.sftp_side = match self.sftp_side {
                    SftpSide::Local => SftpSide::Remote,
                    SftpSide::Remote => SftpSide::Local,
                };
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_sftp(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sftp(1),
            KeyCode::Enter => self.open_sftp_entry(),
            KeyCode::Backspace => self.open_sftp_parent(),
            KeyCode::Char('u') => self.upload_selected(),
            KeyCode::Char('d') => self.download_selected(),
            KeyCode::Char('m') => {
                self.modal = Some(Modal::SftpInput {
                    mkdir: true,
                    side: self.sftp_side,
                    input: TextInput::new(""),
                    entry: None,
                });
            }
            KeyCode::Char('r') => {
                if let Some(entry) = self.selected_sftp_entry().cloned() {
                    self.modal = Some(Modal::SftpInput {
                        mkdir: false,
                        side: self.sftp_side,
                        input: TextInput::new(entry.name.clone()),
                        entry: Some(entry),
                    });
                } else {
                    self.status = "请选择要改名的项目".to_owned();
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(entry) = self.selected_sftp_entry().cloned() {
                    self.modal = Some(Modal::SftpDelete {
                        side: self.sftp_side,
                        entry,
                    });
                } else {
                    self.status = "请选择要删除的项目".to_owned();
                }
            }
            KeyCode::Char('c') => self.cancel_sftp_operation(),
            KeyCode::F(5) => self.refresh_sftp(),
            _ => {}
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.modal = None;
            return;
        }

        let Some(mut modal) = self.modal.take() else {
            return;
        };
        let mut keep = true;
        match &mut modal {
            Modal::Filter(input) => {
                if key.code == KeyCode::Enter {
                    keep = false;
                } else if input.handle_key(key) {
                    self.filter = input.value.clone();
                }
            }
            Modal::Quick(input) => {
                if key.code == KeyCode::Enter {
                    let target = input.value.trim().to_owned();
                    if !target.is_empty() {
                        self.emit(
                            ManagerRequest::Quick {
                                target: target.clone(),
                                where_: self.snapshot.default_where.clone(),
                            },
                            format!("正在连接 {target}…"),
                        );
                    }
                    keep = false;
                } else {
                    input.handle_key(key);
                }
            }
            Modal::Delete { profile } => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.emit(
                        ManagerRequest::Delete {
                            id: profile.id.clone(),
                        },
                        format!("已请求删除 {}", profile.name),
                    );
                    keep = false;
                }
                KeyCode::Char('n') => keep = false,
                _ => {}
            },
            Modal::Edit {
                draft,
                field,
                cursors,
            } => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                    self.modal = Some(modal);
                    self.save_draft();
                    return;
                }
                match key.code {
                    KeyCode::Tab | KeyCode::BackTab => {
                        let delta = if key.code == KeyCode::BackTab
                            || key.modifiers.contains(KeyModifiers::SHIFT)
                        {
                            -1
                        } else {
                            1
                        };
                        *field = move_index(*field, delta, FORM_FIELDS);
                    }
                    KeyCode::Enter => {
                        if *field + 1 == FORM_FIELDS {
                            self.modal = Some(modal);
                            self.save_draft();
                            return;
                        }
                        *field += 1;
                    }
                    _ => {
                        let target = draft_field_mut(draft, *field);
                        let value = std::mem::take(target);
                        let mut input = TextInput {
                            cursor: cursors[*field].min(value.len()),
                            secret: *field == 6,
                            value,
                        };
                        input.handle_key(key);
                        cursors[*field] = input.cursor;
                        *target = input.value;
                    }
                }
            }
            Modal::SftpCredentials {
                profile,
                password,
                jump_password,
                field,
            } => {
                if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                    *field = usize::from(*field == 0);
                } else if key.code == KeyCode::Enter && *field == 0 {
                    *field = 1;
                } else if key.code == KeyCode::Enter {
                    let overrides = ProfileConnectionOverrides {
                        credentials: CredentialOverrides {
                            password: non_empty_string(&password.value),
                            ..CredentialOverrides::default()
                        },
                        jump: CredentialOverrides {
                            password: non_empty_string(&jump_password.value),
                            ..CredentialOverrides::default()
                        },
                        environment: None,
                    };
                    self.establish_sftp(profile.clone(), overrides);
                    keep = false;
                } else if *field == 0 {
                    password.handle_key(key);
                } else {
                    jump_password.handle_key(key);
                }
            }
            Modal::SftpInput {
                mkdir,
                side,
                input,
                entry,
            } => {
                if key.code == KeyCode::Enter {
                    let name = input.value.trim().to_owned();
                    if name.is_empty() {
                        self.status = "名称不能为空".to_owned();
                    } else {
                        let mutation = if *mkdir {
                            Mutation::Mkdir(self.join_panel_path(*side, &name))
                        } else if let Some(entry) = entry {
                            Mutation::Rename {
                                from: entry.path.clone(),
                                to: sibling_path(*side, &entry.path, &name),
                            }
                        } else {
                            self.status = "请选择要改名的项目".to_owned();
                            self.modal = Some(modal);
                            return;
                        };
                        let success = if *mkdir {
                            format!("已创建目录 {name}")
                        } else {
                            format!("已改名为 {name}")
                        };
                        self.start_mutation(*side, mutation, success);
                        keep = false;
                    }
                } else {
                    input.handle_key(key);
                }
            }
            Modal::SftpDelete { side, entry } => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.start_mutation(
                        *side,
                        Mutation::Remove(entry.path.clone()),
                        format!("已删除 {}", entry.name),
                    );
                    keep = false;
                }
                KeyCode::Char('n') => keep = false,
                _ => {}
            },
            Modal::SftpOverwrite {
                upload,
                source,
                destination,
            } => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.perform_transfer(
                        if *upload {
                            TransferDirection::Upload
                        } else {
                            TransferDirection::Download
                        },
                        source.clone(),
                        destination.clone(),
                        true,
                    );
                    keep = false;
                }
                KeyCode::Char('n') => keep = false,
                _ => {}
            },
        }
        if keep {
            self.modal = Some(modal);
        }
        self.clamp_selection();
    }

    fn move_sftp(&mut self, delta: isize) {
        let panel = match self.sftp_side {
            SftpSide::Local => &mut self.local_panel,
            SftpSide::Remote => &mut self.remote_panel,
        };
        panel.selected = move_index(panel.selected, delta, panel.entries.len());
    }

    fn panel(&self, side: SftpSide) -> &SftpPanelView {
        match side {
            SftpSide::Local => &self.local_panel,
            SftpSide::Remote => &self.remote_panel,
        }
    }

    fn panel_mut(&mut self, side: SftpSide) -> &mut SftpPanelView {
        match side {
            SftpSide::Local => &mut self.local_panel,
            SftpSide::Remote => &mut self.remote_panel,
        }
    }

    fn selected_sftp_entry_for(&self, side: SftpSide) -> Option<&SftpEntryView> {
        let panel = self.panel(side);
        panel
            .entries
            .get(panel.selected)
            .filter(|entry| entry.name != "..")
    }

    pub fn selected_sftp_entry(&self) -> Option<&SftpEntryView> {
        self.selected_sftp_entry_for(self.sftp_side)
    }

    fn provider_for(&self, side: SftpSide) -> Option<Arc<dyn FileProvider>> {
        match side {
            SftpSide::Local => Some(Arc::new(self.local_provider)),
            SftpSide::Remote => self
                .remote_provider
                .clone()
                .map(|provider| Arc::new(provider) as Arc<dyn FileProvider>),
        }
    }

    fn read_panel(&mut self, side: SftpSide, path: String) {
        let Some(provider) = self.provider_for(side) else {
            let panel = self.panel_mut(side);
            panel.entries.clear();
            panel.loading = false;
            panel.error = "尚未连接 SFTP".to_owned();
            return;
        };
        let sequence = match side {
            SftpSide::Local => {
                self.local_read_sequence += 1;
                self.local_read_sequence
            }
            SftpSide::Remote => {
                self.remote_read_sequence += 1;
                self.remote_read_sequence
            }
        };
        let panel = self.panel_mut(side);
        panel.path = path.clone();
        panel.entries.clear();
        panel.selected = 0;
        panel.loading = true;
        panel.error.clear();

        let sender = self.event_sender.clone();
        tokio::spawn(async move {
            let options = OperationOptions::default();
            let result = provider
                .list(&path, &options)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(AsyncEvent::PanelLoaded {
                side,
                sequence,
                path,
                result,
            });
        });
    }

    fn refresh_sftp(&mut self) {
        let local = self.local_panel.path.clone();
        self.read_panel(SftpSide::Local, local);
        if self.remote_provider.is_some() {
            let remote = self.remote_panel.path.clone();
            self.read_panel(SftpSide::Remote, remote);
        }
    }

    fn open_sftp_entry(&mut self) {
        let side = self.sftp_side;
        let Some(entry) = self
            .panel(side)
            .entries
            .get(self.panel(side).selected)
            .cloned()
        else {
            return;
        };
        if entry.kind != EntryKind::Directory {
            self.status = format!(
                "{}：按 {} 传输",
                entry.name,
                if side == SftpSide::Local { "u" } else { "d" }
            );
            return;
        }
        self.status = format!(
            "{}：{}",
            if side == SftpSide::Local {
                "本地"
            } else {
                "远端"
            },
            entry.path
        );
        self.read_panel(side, entry.path);
    }

    fn open_sftp_parent(&mut self) {
        let side = self.sftp_side;
        let current = self.panel(side).path.clone();
        let parent = parent_path(side, &current);
        if parent != current {
            self.read_panel(side, parent);
        }
    }

    fn join_panel_path(&self, side: SftpSide, name: &str) -> String {
        join_path(side, &self.panel(side).path, name)
    }

    fn close_sftp_session(&mut self) {
        // Invalidate connection completions and disconnect watchers before the
        // asynchronous close task has a chance to acquire the session mutex.
        self.connection_sequence = self.connection_sequence.wrapping_add(1);
        if let Some(cancel) = self.connection_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = self.operation_cancel.take() {
            cancel.cancel();
        }
        if let Some(session) = self.sftp_session.take() {
            tokio::spawn(async move {
                let _ = session.lock().await.close().await;
            });
        }
        self.remote_provider = None;
        self.connected_profile = None;
        self.connected_fingerprint = None;
        self.sftp_busy = false;
        self.transfer_progress = None;
    }

    fn establish_sftp(&mut self, profile: Profile, overrides: ProfileConnectionOverrides) {
        let mapped = sftp::connection_from_profile(&profile, &self.snapshot.profiles, &overrides);
        let unsupported = mapped
            .issues
            .iter()
            .filter(|issue| issue.severity == CompatibilityIssueSeverity::Unsupported)
            .map(|issue| issue.message.clone())
            .collect::<Vec<_>>();
        if !mapped.supported {
            self.status = unsupported.join("；");
            return;
        }

        self.close_sftp_session();
        let sequence = self.connection_sequence;
        let cancellation = CancellationToken::new();
        self.connection_cancel = Some(cancellation.clone());
        self.sftp_busy = true;
        self.status = format!("正在建立 SFTP：{}…", profile_target(&profile));
        self.remote_panel = SftpPanelView {
            path: "/".to_owned(),
            loading: true,
            ..SftpPanelView::default()
        };

        let sender = self.event_sender.clone();
        let fingerprint = profile_fingerprint(&profile);
        let warnings = mapped
            .issues
            .iter()
            .filter(|issue| issue.severity == CompatibilityIssueSeverity::Warning)
            .map(|issue| issue.message.clone())
            .collect::<Vec<_>>();
        tokio::spawn(async move {
            let operation = OperationOptions::with_cancellation(cancellation.clone());
            let result = match sftp::connect_sftp(&mapped.connection, &operation).await {
                Ok(session) => {
                    let home = session
                        .remote_home(&operation)
                        .await
                        .unwrap_or_else(|_| "/".to_owned());
                    Ok((session, home))
                }
                Err(SftpError::CredentialRequired { role, .. }) => {
                    Err(ConnectFailure::Credential(role))
                }
                Err(SftpError::Aborted) => Err(ConnectFailure::Cancelled),
                Err(error) => Err(ConnectFailure::Message(error.to_string())),
            };
            let _ = sender.send(AsyncEvent::Connected(Box::new(ConnectedEvent {
                sequence,
                profile,
                fingerprint,
                warnings,
                result,
            })));
        });
    }

    fn open_sftp_workspace(&mut self) {
        let Some(profile) = self.selected_host().cloned() else {
            self.status = "请先选择一台主机".to_owned();
            return;
        };
        self.page = MainPage::Sftp;
        let fingerprint = profile_fingerprint(&profile);
        if self.remote_provider.is_some()
            && self
                .connected_profile
                .as_ref()
                .is_some_and(|item| item.id == profile.id)
            && self.connected_fingerprint.as_deref() == Some(fingerprint.as_str())
        {
            let path = self.remote_panel.path.clone();
            self.read_panel(SftpSide::Remote, path);
            return;
        }
        let mapped = sftp::connection_from_profile(
            &profile,
            &self.snapshot.profiles,
            &ProfileConnectionOverrides::default(),
        );
        let unsupported = mapped
            .issues
            .iter()
            .filter(|issue| issue.severity == CompatibilityIssueSeverity::Unsupported)
            .map(|issue| issue.message.clone())
            .collect::<Vec<_>>();
        if !unsupported.is_empty() {
            self.status = unsupported.join("；");
            return;
        }
        let needs_target = mapped.issues.iter().any(|issue| {
            issue.severity == CompatibilityIssueSeverity::NeedsInput
                && !issue.field.starts_with("jump.")
        });
        let needs_jump = mapped.issues.iter().any(|issue| {
            issue.severity == CompatibilityIssueSeverity::NeedsInput
                && issue.field.starts_with("jump.")
        });
        if needs_target || needs_jump {
            self.modal = Some(Modal::SftpCredentials {
                profile,
                password: TextInput::secret(""),
                jump_password: TextInput::secret(""),
                field: usize::from(!needs_target),
            });
            return;
        }
        self.establish_sftp(profile, ProfileConnectionOverrides::default());
    }

    fn upload_selected(&mut self) {
        let Some(entry) = self.selected_sftp_entry_for(SftpSide::Local).cloned() else {
            self.status = "请选择一个本地文件".to_owned();
            return;
        };
        if entry.kind != EntryKind::File {
            self.status = "当前仅传输普通文件，目录和符号链接请先打包".to_owned();
            return;
        }
        let destination = join_path(SftpSide::Remote, &self.remote_panel.path, &entry.name);
        self.perform_transfer(TransferDirection::Upload, entry.path, destination, false);
    }

    fn download_selected(&mut self) {
        let Some(entry) = self.selected_sftp_entry_for(SftpSide::Remote).cloned() else {
            self.status = "请选择一个远端文件".to_owned();
            return;
        };
        if entry.kind != EntryKind::File {
            self.status = "当前仅传输普通文件，目录和符号链接请先打包".to_owned();
            return;
        }
        let destination = join_path(SftpSide::Local, &self.local_panel.path, &entry.name);
        self.perform_transfer(TransferDirection::Download, entry.path, destination, false);
    }

    fn perform_transfer(
        &mut self,
        direction: TransferDirection,
        source: String,
        destination: String,
        overwrite: bool,
    ) {
        let Some(session) = self.sftp_session.clone() else {
            self.status = "请先连接 SFTP".to_owned();
            return;
        };
        if self.sftp_busy {
            self.status = "已有 SFTP 操作正在进行".to_owned();
            return;
        }
        let cancellation = CancellationToken::new();
        self.operation_cancel = Some(cancellation.clone());
        self.sftp_busy = true;
        self.transfer_progress = None;
        let sender = self.event_sender.clone();
        let progress_sender = sender.clone();
        let progress = Arc::new(move |progress: TransferProgress| {
            let _ = progress_sender.send(AsyncEvent::TransferProgress(progress));
        });
        let source_for_task = source.clone();
        let destination_for_task = destination.clone();
        tokio::spawn(async move {
            let options = TransferOptions {
                operation: OperationOptions::with_cancellation(cancellation),
                overwrite,
                preserve_times: true,
                on_progress: Some(progress),
                ..TransferOptions::default()
            };
            let result = {
                let session = session.lock().await;
                match direction {
                    TransferDirection::Upload => {
                        session
                            .upload(&source_for_task, &destination_for_task, &options)
                            .await
                    }
                    TransferDirection::Download => {
                        session
                            .download(&source_for_task, &destination_for_task, &options)
                            .await
                    }
                }
            };
            let outcome = match result {
                Ok(()) => TransferOutcome::Completed,
                Err(SftpError::DestinationExists(_)) => TransferOutcome::DestinationExists,
                Err(SftpError::Aborted) => TransferOutcome::Cancelled,
                Err(error) => TransferOutcome::Failed(error.to_string()),
            };
            let _ = sender.send(AsyncEvent::TransferFinished {
                direction,
                source,
                destination,
                outcome,
            });
        });
    }

    fn start_mutation(&mut self, side: SftpSide, mutation: Mutation, success: String) {
        let Some(provider) = self.provider_for(side) else {
            self.status = "请先连接 SFTP".to_owned();
            return;
        };
        if self.sftp_busy {
            self.status = "已有 SFTP 操作正在进行".to_owned();
            return;
        }
        let cancellation = CancellationToken::new();
        self.operation_cancel = Some(cancellation.clone());
        self.sftp_busy = true;
        let sender = self.event_sender.clone();
        tokio::spawn(async move {
            let operation = OperationOptions::with_cancellation(cancellation);
            let result = match mutation {
                Mutation::Mkdir(path) => {
                    provider
                        .mkdir(
                            &path,
                            &MkdirOptions {
                                operation,
                                recursive: false,
                                mode: None,
                            },
                        )
                        .await
                }
                Mutation::Rename { from, to } => provider.rename(&from, &to, &operation).await,
                Mutation::Remove(path) => {
                    provider
                        .remove(
                            &path,
                            &RemoveOptions {
                                operation,
                                recursive: true,
                            },
                        )
                        .await
                }
            };
            let _ = sender.send(AsyncEvent::MutationFinished {
                side,
                success,
                result: result.map_err(|error| error.to_string()),
            });
        });
    }

    fn cancel_sftp_operation(&mut self) {
        if let Some(cancel) = self.connection_cancel.as_ref() {
            cancel.cancel();
        } else if let Some(cancel) = self.operation_cancel.as_ref() {
            cancel.cancel();
        } else {
            self.status = "当前没有可取消的操作".to_owned();
            return;
        }
        self.status = "正在取消…".to_owned();
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }
        match self.page {
            MainPage::Manager => self.handle_manager_key(key),
            MainPage::Sftp => self.handle_sftp_key(key),
        }
        self.clamp_selection();
    }

    fn handle_paste(&mut self, text: &str) {
        let mut next_filter = None;
        match self.modal.as_mut() {
            Some(Modal::Filter(input)) => {
                input.insert_text(text);
                next_filter = Some(input.value.clone());
            }
            Some(Modal::Quick(input)) | Some(Modal::SftpInput { input, .. }) => {
                input.insert_text(text);
            }
            Some(Modal::SftpCredentials {
                password,
                jump_password,
                field,
                ..
            }) => {
                if *field == 0 {
                    password.insert_text(text);
                } else {
                    jump_password.insert_text(text);
                }
            }
            Some(Modal::Edit {
                draft,
                field,
                cursors,
            }) => {
                let target = draft_field_mut(draft, *field);
                let value = std::mem::take(target);
                let mut input = TextInput {
                    cursor: cursors[*field].min(value.len()),
                    secret: *field == 6,
                    value,
                };
                input.insert_text(text);
                cursors[*field] = input.cursor;
                *target = input.value;
            }
            Some(Modal::Delete { .. })
            | Some(Modal::SftpDelete { .. })
            | Some(Modal::SftpOverwrite { .. })
            | None => {}
        }
        if let Some(filter) = next_filter {
            self.filter = filter;
        }
        self.clamp_selection();
    }

    fn handle_resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
        if !self.show_details() && self.focus == ManagerFocus::Details {
            self.focus = ManagerFocus::Hosts;
        }
        self.clamp_selection();
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        if let Some(modal) = self.modal.as_mut() {
            if let Modal::Edit { field, .. } = modal
                && let Some(clicked) =
                    ui::edit_modal_field_at(self.terminal_width, self.terminal_height, point)
            {
                *field = clicked;
            }
            return;
        }
        match self.page {
            MainPage::Manager => {
                let regions = ui::manager_regions(self.terminal_width, self.terminal_height);
                if let Some(row) = ui::manager_group_row_at(regions.groups, point) {
                    if row < self.groups().len() {
                        self.focus = ManagerFocus::Groups;
                        self.group_index = row;
                        self.host_index = 0;
                    }
                } else if let Some(row) = ui::manager_host_row_at(regions.hosts, point) {
                    let visible = self.visible_host_window();
                    let clicked = visible.get(row).map(|(index, _)| *index);
                    drop(visible);
                    if let Some(index) = clicked {
                        self.focus = ManagerFocus::Hosts;
                        self.host_index = index;
                        let now = Instant::now();
                        let double = self
                            .last_host_click
                            .map(|(previous, at)| {
                                previous == index
                                    && now.duration_since(at) <= Duration::from_millis(450)
                            })
                            .unwrap_or(false);
                        self.last_host_click = Some((index, now));
                        if double {
                            self.last_host_click = None;
                            self.connect(self.snapshot.default_where.clone());
                        }
                    }
                } else if regions.details.is_some_and(|area| area.contains(point)) {
                    self.focus = ManagerFocus::Details;
                }
            }
            MainPage::Sftp => {
                let regions = ui::sftp_regions(self.terminal_width, self.terminal_height);
                let (side, area) = if regions.local.contains(point) {
                    (SftpSide::Local, regions.local)
                } else if regions.remote.contains(point) {
                    (SftpSide::Remote, regions.remote)
                } else {
                    return;
                };
                self.sftp_side = side;
                let panel = match side {
                    SftpSide::Local => &mut self.local_panel,
                    SftpSide::Remote => &mut self.remote_panel,
                };
                let row = mouse.row.saturating_sub(area.y + 3) as usize;
                if row < panel.entries.len() {
                    panel.selected = row;
                }
            }
        }
    }

    pub fn visible_host_window(&self) -> Vec<(usize, &Profile)> {
        let hosts = self.hosts();
        let count = usize::from(self.terminal_height.saturating_sub(10)).max(4);
        let start = self
            .host_index
            .saturating_sub(count / 2)
            .min(hosts.len().saturating_sub(count));
        hosts
            .into_iter()
            .enumerate()
            .skip(start)
            .take(count)
            .collect()
    }

    fn handle_async(&mut self, event: AsyncEvent) {
        match event {
            AsyncEvent::PanelLoaded {
                side,
                sequence,
                path,
                result,
            } => {
                let current = match side {
                    SftpSide::Local => self.local_read_sequence,
                    SftpSide::Remote => self.remote_read_sequence,
                };
                if sequence != current {
                    return;
                }
                match result {
                    Ok(entries) => {
                        let mut entries = entries.into_iter().map(entry_view).collect::<Vec<_>>();
                        let parent = parent_path(side, &path);
                        if parent != path {
                            entries.insert(
                                0,
                                SftpEntryView {
                                    name: "..".to_owned(),
                                    path: parent,
                                    kind: EntryKind::Directory,
                                    size: None,
                                    modified: None,
                                },
                            );
                        }
                        *self.panel_mut(side) = SftpPanelView {
                            path,
                            entries,
                            selected: 0,
                            loading: false,
                            error: String::new(),
                        };
                    }
                    Err(error) => {
                        let panel = self.panel_mut(side);
                        panel.path = path;
                        panel.entries.clear();
                        panel.selected = 0;
                        panel.loading = false;
                        panel.error = error.clone();
                        self.status = error;
                    }
                }
            }
            AsyncEvent::Connected(event) => {
                let ConnectedEvent {
                    sequence,
                    profile,
                    fingerprint,
                    warnings,
                    result,
                } = *event;
                if sequence != self.connection_sequence {
                    if let Ok((mut session, _)) = result {
                        tokio::spawn(async move {
                            let _ = session.close().await;
                        });
                    }
                    return;
                }
                self.connection_cancel = None;
                self.sftp_busy = false;
                if self
                    .snapshot
                    .profiles
                    .iter()
                    .find(|item| item.id == profile.id)
                    .is_none_or(|item| profile_fingerprint(item) != fingerprint)
                {
                    if let Ok((mut session, _)) = result {
                        tokio::spawn(async move {
                            let _ = session.close().await;
                        });
                    }
                    self.status = "连接期间配置已更新，请重试".to_owned();
                    return;
                }
                match result {
                    Ok((session, home)) => {
                        let remote = session.remote.clone();
                        let mut disconnected = session.disconnect_receiver();
                        let sender = self.event_sender.clone();
                        tokio::spawn(async move {
                            while disconnected.changed().await.is_ok() {
                                if let Some(message) = disconnected.borrow().clone() {
                                    let _ = sender.send(AsyncEvent::Disconnected {
                                        sequence,
                                        detail: message,
                                    });
                                    break;
                                }
                            }
                        });
                        self.remote_provider = Some(remote);
                        self.sftp_session = Some(Arc::new(Mutex::new(session)));
                        self.connected_profile = Some(profile.clone());
                        self.connected_fingerprint = Some(fingerprint);
                        self.status = if warnings.is_empty() {
                            format!("SFTP 已连接：{}", profile.name)
                        } else {
                            format!("SFTP 已连接；{}", warnings.join("；"))
                        };
                        self.remote_panel = SftpPanelView {
                            path: home.clone(),
                            ..SftpPanelView::default()
                        };
                        self.read_panel(SftpSide::Remote, home);
                    }
                    Err(ConnectFailure::Credential(role)) => {
                        self.status = format!(
                            "{}需要认证信息",
                            if role == ConnectionRole::Jump {
                                "跳板机"
                            } else {
                                "目标主机"
                            }
                        );
                        self.modal = Some(Modal::SftpCredentials {
                            profile,
                            password: TextInput::secret(""),
                            jump_password: TextInput::secret(""),
                            field: usize::from(role == ConnectionRole::Jump),
                        });
                    }
                    Err(ConnectFailure::Cancelled) => {
                        self.status = "SFTP 连接已取消".to_owned();
                        self.remote_panel.loading = false;
                    }
                    Err(ConnectFailure::Message(error)) => {
                        self.status = error.clone();
                        self.remote_panel.loading = false;
                        self.remote_panel.error = error;
                    }
                }
            }
            AsyncEvent::Disconnected { sequence, detail } => {
                if sequence != self.connection_sequence {
                    return;
                }
                self.close_sftp_session();
                let message = if detail.is_empty() {
                    "SFTP 连接已断开".to_owned()
                } else {
                    format!("SFTP 连接已断开：{detail}")
                };
                self.remote_panel.entries.clear();
                self.remote_panel.loading = false;
                self.remote_panel.error = message.clone();
                self.status = message;
            }
            AsyncEvent::TransferProgress(progress) => {
                let text = progress_text(&progress);
                self.transfer_progress = Some(text.clone());
                self.status = text;
            }
            AsyncEvent::TransferFinished {
                direction,
                source,
                destination,
                outcome,
            } => {
                self.operation_cancel = None;
                self.sftp_busy = false;
                self.transfer_progress = None;
                match outcome {
                    TransferOutcome::Completed => {
                        self.status = format!(
                            "{}完成：{}",
                            if direction == TransferDirection::Upload {
                                "上传"
                            } else {
                                "下载"
                            },
                            path_basename(&destination)
                        );
                        self.refresh_sftp();
                    }
                    TransferOutcome::DestinationExists => {
                        self.modal = Some(Modal::SftpOverwrite {
                            upload: direction == TransferDirection::Upload,
                            source,
                            destination,
                        });
                        self.status = "目标已存在，请确认是否覆盖".to_owned();
                    }
                    TransferOutcome::Cancelled => {
                        self.status = "传输已取消".to_owned();
                    }
                    TransferOutcome::Failed(error) => self.status = error,
                }
            }
            AsyncEvent::MutationFinished {
                side,
                success,
                result,
            } => {
                self.operation_cancel = None;
                self.sftp_busy = false;
                match result {
                    Ok(()) => {
                        self.status = success;
                        let path = self.panel(side).path.clone();
                        self.read_panel(side, path);
                    }
                    Err(error) => self.status = error,
                }
            }
        }
    }

    async fn refresh_snapshot(&mut self) {
        let metadata = match tokio::fs::metadata(&self.snapshot_path).await {
            Ok(metadata) => metadata,
            Err(error) => {
                self.status = format!("snapshot：{error}");
                return;
            }
        };
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(error) => {
                self.status = format!("snapshot：{error}");
                return;
            }
        };
        if self
            .last_snapshot_modified
            .is_some_and(|previous| modified <= previous)
        {
            return;
        }
        match read_snapshot(&self.snapshot_path) {
            Ok(snapshot) => {
                self.last_snapshot_modified = Some(modified);
                let count = snapshot.profiles.len();
                let connected = self
                    .connected_profile
                    .as_ref()
                    .map(|profile| (profile.id.clone(), self.connected_fingerprint.clone()));
                self.snapshot = snapshot;
                self.clamp_selection();
                self.status = format!("已同步 {count} 台主机");
                if let Some((id, fingerprint)) = connected
                    && self
                        .snapshot
                        .profiles
                        .iter()
                        .find(|profile| profile.id == id)
                        .is_none_or(|profile| Some(profile_fingerprint(profile)) != fingerprint)
                {
                    self.close_sftp_session();
                    self.remote_panel.entries.clear();
                    self.remote_panel.loading = false;
                    self.remote_panel.error = "配置已更新，请重新连接".to_owned();
                    self.status = "当前 SFTP 配置已更新，旧连接已关闭".to_owned();
                }
            }
            Err(error) => self.status = format!("snapshot：{error}"),
        }
    }
}

pub async fn run(
    snapshot_path: PathBuf,
    initial_snapshot: Snapshot,
    protocol: RequestProtocol,
) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let _restore = TerminalRestore;
    let (event_sender, mut async_events) = mpsc::unbounded_channel();
    let mut app = App::new(snapshot_path, initial_snapshot, protocol, event_sender);
    app.read_panel(SftpSide::Local, app.local_panel.path.clone());
    let mut events = EventStream::new();
    let mut snapshot_tick = interval(Duration::from_millis(400));
    snapshot_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.terminal_width = area.width;
                app.terminal_height = area.height;
                ui::draw(frame, &app);
            })
            .context("cannot draw TUI")?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => app.handle_key(key),
                Some(Ok(Event::Mouse(mouse))) => app.handle_mouse(mouse),
                Some(Ok(Event::Paste(text))) => app.handle_paste(&text),
                Some(Ok(Event::Resize(width, height))) => app.handle_resize(width, height),
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error).context("terminal event failed"),
                None => break,
            },
            _ = snapshot_tick.tick() => app.refresh_snapshot().await,
            Some(event) = async_events.recv() => app.handle_async(event),
        }
    }
    if let Some(cancellation) = app.connection_cancel.take() {
        cancellation.cancel();
    }
    if let Some(cancellation) = app.operation_cancel.take() {
        cancellation.cancel();
    }
    if let Some(session) = app.sftp_session.take() {
        let _ = session.lock().await.close().await;
    }
    Ok(())
}

fn non_empty_string(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn profile_fingerprint(profile: &Profile) -> String {
    serde_json::to_string(profile).unwrap_or_else(|_| profile.id.clone())
}

fn entry_view(entry: FileEntry) -> SftpEntryView {
    let kind = match entry.kind {
        FileKind::Directory => EntryKind::Directory,
        FileKind::File => EntryKind::File,
        FileKind::Symlink => EntryKind::Link,
        FileKind::Other => EntryKind::Other,
    };
    let modified = entry.modified_at.map(|value| {
        let local: DateTime<Local> = value.into();
        local.format("%m-%d %H:%M").to_string()
    });
    SftpEntryView {
        name: entry.name,
        path: entry.path,
        kind,
        size: (kind != EntryKind::Directory).then_some(entry.size),
        modified,
    }
}

fn join_path(side: SftpSide, base: &str, name: &str) -> String {
    match side {
        SftpSide::Local => Path::new(base).join(name).to_string_lossy().into_owned(),
        SftpSide::Remote => {
            let base = base.trim_end_matches('/');
            if base.is_empty() {
                format!("/{}", name.trim_start_matches('/'))
            } else {
                format!("{base}/{}", name.trim_start_matches('/'))
            }
        }
    }
}

fn parent_path(side: SftpSide, current: &str) -> String {
    match side {
        SftpSide::Local => Path::new(current)
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| current.to_owned()),
        SftpSide::Remote => {
            let trimmed = current.trim_end_matches('/');
            if trimmed.is_empty() {
                return "/".to_owned();
            }
            match trimmed.rfind('/') {
                Some(0) => "/".to_owned(),
                Some(index) => trimmed[..index].to_owned(),
                None => ".".to_owned(),
            }
        }
    }
}

fn sibling_path(side: SftpSide, current: &str, name: &str) -> String {
    join_path(side, &parent_path(side, current), name)
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn progress_text(progress: &TransferProgress) -> String {
    let verb = if progress.direction == TransferDirection::Upload {
        "上传"
    } else {
        "下载"
    };
    let amount = progress.total_bytes.map_or_else(
        || format!("{} B", progress.transferred_bytes),
        |total| format!("{}/{} B", progress.transferred_bytes, total),
    );
    let percent = progress
        .percent
        .map(|value| format!(" {value:.0}%"))
        .unwrap_or_default();
    let speed = if progress.bytes_per_second > 0.0 {
        format!(" · {:.1} KiB/s", progress.bytes_per_second / 1024.0)
    } else {
        String::new()
    };
    format!(
        "{verb} {} · {amount}{percent}{speed}",
        path_basename(&progress.source)
    )
}

fn draft_field_mut(draft: &mut ProfileDraft, index: usize) -> &mut String {
    match index {
        0 => &mut draft.name,
        1 => &mut draft.group,
        2 => &mut draft.host,
        3 => &mut draft.port,
        4 => &mut draft.user,
        5 => &mut draft.auth,
        6 => &mut draft.password,
        _ => &mut draft.jump_host,
    }
}

fn draft_field(draft: &ProfileDraft, index: usize) -> &str {
    match index {
        0 => &draft.name,
        1 => &draft.group,
        2 => &draft.host,
        3 => &draft.port,
        4 => &draft.user,
        5 => &draft.auth,
        6 => &draft.password,
        _ => &draft.jump_host,
    }
}

fn clamp_index(index: usize, len: usize) -> usize {
    index.min(len.saturating_sub(1))
}

fn move_index(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    index.saturating_add_signed(delta).min(len - 1)
}

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<TuiTerminal> {
    enable_raw_mode().context("cannot enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("cannot enter alternate screen")?;
    Terminal::new(CrosstermBackend::new(stdout)).context("cannot initialize terminal")
}

struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        model::normalize_snapshot,
        runtime::{RuntimeContext, cleanup_runtime, create_runtime, is_request_filename},
    };

    fn sample_snapshot(profile_count: usize) -> Snapshot {
        let profiles = (0..profile_count)
            .map(|index| {
                json!({
                    "id": format!("host-{index}"),
                    "name": format!("主机-{index}"),
                    "group": if index % 2 == 0 { "alpha" } else { "beta" },
                    "editable": true,
                    "host": format!("192.0.2.{}", index + 1),
                    "port": 22,
                    "raw": {
                        "name": format!("主机-{index}"),
                        "group": if index % 2 == 0 { "alpha" } else { "beta" },
                        "host": format!("192.0.2.{}", index + 1)
                    }
                })
            })
            .collect::<Vec<_>>();
        normalize_snapshot(&json!({
            "store_path": "/tmp/profiles.lua",
            "default_where": "tab",
            "groups": ["alpha", "beta"],
            "profiles": profiles,
        }))
    }

    fn test_app(snapshot: Snapshot) -> (App, RuntimeContext) {
        let runtime = create_runtime().unwrap();
        let snapshot_path = runtime.runtime_dir.join("snapshot.json");
        fs::write(&snapshot_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let protocol =
            RequestProtocol::with_writer(&runtime.runtime_dir, &runtime.token, |_| Ok(())).unwrap();
        let (event_sender, _events) = mpsc::unbounded_channel();
        (
            App::new(snapshot_path, snapshot, protocol, event_sender),
            runtime,
        )
    }

    fn request_bodies(runtime: &RuntimeContext) -> Vec<Value> {
        let mut requests = fs::read_dir(&runtime.runtime_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_str().is_some_and(is_request_filename))
            .map(|entry| serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap())
            .collect::<Vec<Value>>();
        requests.sort_by_key(|request| request["_seq"].as_u64());
        requests
    }

    #[test]
    fn text_input_edits_graphemes_without_splitting_utf8() {
        let mut input = TextInput::new("主机🙂");
        input.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.value, "主机");
        input.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        input.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.value, "机");
    }

    #[test]
    fn index_movement_is_clamped() {
        assert_eq!(move_index(0, -1, 3), 0);
        assert_eq!(move_index(2, 1, 3), 2);
        assert_eq!(move_index(4, -1, 0), 0);
    }

    #[test]
    fn edit_form_preserves_cursor_and_accepts_paste() {
        let (mut app, runtime) = test_app(sample_snapshot(1));
        let profile = app.selected_host().cloned();
        app.open_edit(profile);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
        app.handle_paste("粘贴");

        let Some(Modal::Edit {
            draft,
            field,
            cursors,
        }) = app.modal.as_ref()
        else {
            panic!("edit modal expected");
        };
        assert_eq!(*field, 0);
        assert_eq!(draft.name, "主机-X粘贴0");
        assert_eq!(cursors[0], "主机-X粘贴".len());

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert!(matches!(app.modal, Some(Modal::Edit { field: 0, .. })));

        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let requests = request_bodies(&runtime);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "upsert");
        assert_eq!(requests[0]["id"], "host-0");
        assert_eq!(requests[0]["raw"]["name"], "主机-X粘贴0");
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn live_filter_preserves_valid_selection_and_clamps_when_needed() {
        let (mut app, runtime) = test_app(sample_snapshot(5));
        app.host_index = 3;
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('主'), KeyModifiers::NONE));
        assert_eq!(app.host_index, 3);
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.host_index, 3);
        app.handle_paste("host-4");
        assert_eq!(app.host_index, 0);
        assert_eq!(app.hosts()[0].id, "host-4");
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn manager_emits_compatible_connect_and_quick_requests() {
        let (mut app, runtime) = test_app(sample_snapshot(1));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        app.handle_paste("ops@example.com:2200");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        let requests = request_bodies(&runtime);
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0],
            json!({
                "op": "connect",
                "id": "host-0",
                "where": "window",
                "_session": runtime.token,
                "_seq": 1,
            })
        );
        assert_eq!(requests[1]["op"], "quick");
        assert_eq!(requests[1]["target"], "ops@example.com:2200");
        assert_eq!(requests[1]["where"], "tab");
        assert_eq!(requests[2]["op"], "hide");
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn manager_mouse_ignores_borders_and_header_then_maps_visible_row() {
        let (mut app, runtime) = test_app(sample_snapshot(10));
        app.handle_resize(120, 14);
        app.host_index = 6;
        let regions = ui::manager_regions(app.terminal_width, app.terminal_height);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(click(regions.hosts.x + 1, regions.hosts.y));
        app.handle_mouse(click(regions.hosts.x + 1, regions.hosts.y + 1));
        assert_eq!(app.host_index, 6);
        app.handle_mouse(click(regions.hosts.x + 1, regions.hosts.y + 2));
        assert_eq!(app.host_index, 4);

        app.group_index = 2;
        app.handle_mouse(click(regions.groups.x + 1, regions.groups.y));
        assert_eq!(app.group_index, 2);
        app.handle_mouse(click(regions.groups.x + 1, regions.groups.y + 1));
        assert_eq!(app.group_index, 0);
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[test]
    fn narrow_resize_moves_focus_out_of_hidden_details_and_draws() {
        let (mut app, runtime) = test_app(sample_snapshot(2));
        app.focus = ManagerFocus::Details;
        app.handle_resize(80, 4);
        assert_eq!(app.focus, ManagerFocus::Hosts);

        app.open_edit(None);
        for (width, height) in [(1, 1), (20, 4), (80, 10)] {
            app.handle_resize(width, height);
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| ui::draw(frame, &app)).unwrap();
        }
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }

    #[tokio::test]
    async fn snapshot_refresh_updates_state_and_reports_read_errors() {
        let (mut app, runtime) = test_app(sample_snapshot(1));
        fs::write(
            &app.snapshot_path,
            serde_json::to_vec(&sample_snapshot(2)).unwrap(),
        )
        .unwrap();
        app.refresh_snapshot().await;
        assert_eq!(app.snapshot.profiles.len(), 2);
        assert_eq!(app.status, "已同步 2 台主机");

        fs::remove_file(&app.snapshot_path).unwrap();
        app.refresh_snapshot().await;
        assert!(app.status.starts_with("snapshot："));
        cleanup_runtime(runtime.runtime_dir).unwrap();
    }
}
