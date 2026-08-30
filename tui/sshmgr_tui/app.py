"""Left-group / right-host SSH manager, plus an edit form."""
from __future__ import annotations

import copy
import json
import os
from typing import Callable, Optional

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import ModalScreen, Screen
from textual.widgets import (
    Button,
    DataTable,
    Footer,
    Header,
    Input,
    Label,
    ListItem,
    ListView,
    Select,
    Static,
)

from .fields import (
    FIELDS,
    format_list,
    get_path,
    parse_list,
    parse_target,
    resolve_field_path,
    set_path,
)
from .protocol import emit

ALL = '__all__'
UNSET = '__unset__'


def _load(path: str) -> dict:
    with open(path, encoding='utf-8') as f:
        data = json.load(f)
    if not isinstance(data, dict):
        return {'profiles': [], 'groups': []}
    profiles = data.get('profiles') or []
    if isinstance(profiles, dict):
        profiles = list(profiles.values())
    data['profiles'] = profiles
    groups = data.get('groups') or []
    if isinstance(groups, dict):
        groups = list(groups.values())
    data['groups'] = groups
    return data


def _target(p: dict) -> str:
    user = p.get('user') or ''
    host = p.get('host') or '?'
    port = p.get('port') or 22
    s = f'{user}@{host}' if user else str(host)
    if port and port != 22:
        s += f':{port}'
    return s


def _matches(p: dict, needle: str) -> bool:
    if not needle:
        return True
    n = needle.lower()
    hay = ' '.join(
        str(x or '')
        for x in (p.get('id'), p.get('name'), p.get('group'), p.get('host'), p.get('user'), p.get('jumpHost'))
    )
    return n in hay.lower()


# ---------------------------------------------------------------------------
# modals
# ---------------------------------------------------------------------------


class ConfirmModal(ModalScreen[bool]):
    BINDINGS = [Binding('escape', 'cancel', show=False), Binding('enter', 'ok', show=False)]

    def __init__(self, message: str) -> None:
        super().__init__()
        self.message = message

    def compose(self) -> ComposeResult:
        with Vertical(id='modal'):
            yield Label(self.message)
            with Horizontal(id='modal-btns'):
                yield Button('确定', id='ok', variant='error')
                yield Button('取消', id='cancel')

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == 'ok')

    def action_ok(self) -> None:
        self.dismiss(True)

    def action_cancel(self) -> None:
        self.dismiss(False)


class PromptModal(ModalScreen[Optional[str]]):
    BINDINGS = [Binding('escape', 'cancel', show=False)]

    def __init__(self, title: str, placeholder: str = '', initial: str = '') -> None:
        super().__init__()
        self._title = title
        self._placeholder = placeholder
        self._initial = initial

    def compose(self) -> ComposeResult:
        with Vertical(id='modal'):
            yield Label(self._title)
            yield Input(value=self._initial, placeholder=self._placeholder, id='prompt')
            with Horizontal(id='modal-btns'):
                yield Button('确定', id='ok', variant='primary')
                yield Button('取消', id='cancel')

    def on_mount(self) -> None:
        self.query_one('#prompt', Input).focus()

    def on_input_submitted(self) -> None:
        self.dismiss(self.query_one('#prompt', Input).value)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == 'ok':
            self.dismiss(self.query_one('#prompt', Input).value)
        else:
            self.dismiss(None)

    def action_cancel(self) -> None:
        self.dismiss(None)


# ---------------------------------------------------------------------------
# scripts / forwards editors
# ---------------------------------------------------------------------------


class ScriptsScreen(Screen):
    BINDINGS = [
        Binding('n', 'add', '添加'),
        Binding('d', 'delete', '删除'),
        Binding('escape', 'back', '返回'),
    ]

    def __init__(self, steps: list, on_done: Callable[[list], None]) -> None:
        super().__init__()
        self.steps = copy.deepcopy(steps or [])
        self.on_done = on_done

    def compose(self) -> ComposeResult:
        yield Header(show_clock=False)
        yield Static('登录脚本  ·  n 添加  d 删除  Enter 编辑  Esc 返回', id='hint')
        yield DataTable(id='steps', cursor_type='row')
        yield Footer()

    def on_mount(self) -> None:
        self.refresh_rows()

    def refresh_rows(self) -> None:
        table = self.query_one('#steps', DataTable)
        table.clear(columns=True)
        table.add_columns('#', '等待', '发送')
        for i, s in enumerate(self.steps):
            if isinstance(s, str):
                expect, send = '', s
            else:
                expect = s.get('expect') or '(立即)'
                send = s.get('send') or ''
            table.add_row(str(i + 1), expect, send, key=str(i))

    def _idx(self) -> Optional[int]:
        table = self.query_one('#steps', DataTable)
        try:
            return int(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value)
        except Exception:
            return None

    def action_add(self) -> None:
        self.steps.append({'expect': '', 'send': ''})
        self.refresh_rows()
        self._edit(len(self.steps) - 1)

    def action_delete(self) -> None:
        i = self._idx()
        if i is None:
            return
        self.steps.pop(i)
        self.refresh_rows()

    def on_data_table_row_selected(self) -> None:
        i = self._idx()
        if i is not None:
            self._edit(i)

    def _edit(self, i: int) -> None:
        original = self.steps[i]
        if isinstance(original, str):
            draft = {'expect': '', 'send': original}
        else:
            draft = copy.deepcopy(original)

        def after_expect(line: Optional[str]) -> None:
            if line is None:
                return
            draft['expect'] = line

            def after_send(send: Optional[str]) -> None:
                if send is None:
                    return
                draft['send'] = send
                if isinstance(original, str) and not draft.get('expect'):
                    self.steps[i] = send
                else:
                    self.steps[i] = draft
                self.refresh_rows()

            self.app.push_screen(
                PromptModal(
                    '要发送的内容（支持 ${password} ${user} ${host}）',
                    initial=str(draft.get('send') or ''),
                ),
                after_send,
            )
        self.app.push_screen(
            PromptModal(
                '等待屏幕出现的文本（留空 = 立即发送）',
                initial=str(draft.get('expect') or ''),
            ),
            after_expect,
        )

    def action_back(self) -> None:
        self.on_done(self.steps)
        self.app.pop_screen()


class ForwardsScreen(Screen):
    BINDINGS = [
        Binding('n', 'add', '添加'),
        Binding('d', 'delete', '删除'),
        Binding('escape', 'back', '返回'),
    ]

    def __init__(self, fwds: list, on_done: Callable[[list], None]) -> None:
        super().__init__()
        self.fwds = copy.deepcopy(fwds or [])
        self.on_done = on_done

    def compose(self) -> ComposeResult:
        yield Header(show_clock=False)
        yield Static('端口转发  ·  n 添加  d 删除  Esc 返回', id='hint')
        yield DataTable(id='fwds', cursor_type='row')
        yield Footer()

    def on_mount(self) -> None:
        self.refresh_rows()

    def refresh_rows(self) -> None:
        table = self.query_one('#fwds', DataTable)
        table.clear(columns=True)
        table.add_columns('#', '类型', '说明')
        for i, f in enumerate(self.fwds):
            if isinstance(f, str):
                desc, kind = f, '?'
            elif f.get('type') == 'Dynamic':
                kind = 'Dynamic'
                desc = f"SOCKS {f.get('host') or '*'}:{f.get('port')}"
            else:
                kind = f.get('type') or 'Local'
                desc = f"{f.get('host') or '*'}:{f.get('port')} → {f.get('targetAddress')}:{f.get('targetPort')}"
            table.add_row(str(i + 1), kind, desc, key=str(i))

    def _idx(self) -> Optional[int]:
        table = self.query_one('#fwds', DataTable)
        try:
            return int(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value)
        except Exception:
            return None

    def action_delete(self) -> None:
        i = self._idx()
        if i is None:
            return
        self.fwds.pop(i)
        self.refresh_rows()

    def action_add(self) -> None:
        def after_kind(line: Optional[str]) -> None:
            if not line:
                return
            kind = line.strip()
            if kind not in ('Local', 'Remote', 'Dynamic'):
                kind = 'Local'
            hint = '本地端口，例如 1080' if kind == 'Dynamic' else '本地端口:目标主机:目标端口，例如 15432:db:5432'
            def after_spec(spec: Optional[str]) -> None:
                if not spec or not spec.strip():
                    return
                parts = spec.strip().split(':')
                if kind == 'Dynamic':
                    try:
                        port = int(parts[0])
                    except ValueError:
                        return
                    self.fwds.append({'type': kind, 'host': '127.0.0.1', 'port': port})
                else:
                    if len(parts) < 3:
                        return
                    try:
                        port = int(parts[0])
                        tport = int(parts[-1])
                    except ValueError:
                        return
                    self.fwds.append({
                        'type': kind,
                        'host': '127.0.0.1',
                        'port': port,
                        'targetAddress': parts[1],
                        'targetPort': tport,
                    })
                self.refresh_rows()
            self.app.push_screen(PromptModal(hint), after_spec)
        self.app.push_screen(PromptModal('类型：Local / Remote / Dynamic', initial='Local'), after_kind)

    def action_back(self) -> None:
        self.on_done(self.fwds)
        self.app.pop_screen()


# ---------------------------------------------------------------------------
# edit form
# ---------------------------------------------------------------------------


class EditScreen(Screen):
    BINDINGS = [
        Binding('ctrl+s', 'save', '保存', show=True),
        Binding('escape', 'back', '取消'),
    ]

    def __init__(self, profile: dict, original_id: Optional[str]) -> None:
        super().__init__()
        self.profile = profile
        self.original_id = original_id
        self.raw: dict = copy.deepcopy(profile.get('raw') or {
            'name': profile.get('name') or '',
            'group': profile.get('group') or '',
            'options': {
                'host': profile.get('host'),
                'user': profile.get('user'),
                'port': profile.get('port'),
            },
        })
        self._field_paths = {
            spec['key']: resolve_field_path(self.raw, spec['key']) for spec in FIELDS
        }
        self._initial_values: dict[str, object] = {}

    def _field_path(self, key: str) -> str:
        return self._field_paths.get(key, key)

    def _field_value(self, key: str):
        return get_path(self.raw, self._field_path(key))

    def compose(self) -> ComposeResult:
        yield Header(show_clock=False)
        title = self.raw.get('name') or '(新连接)'
        yield Static(f'编辑  ·  {title}  ·  Ctrl+S 保存  Esc 取消', id='hint')
        with VerticalScroll(id='form'):
            for spec in FIELDS:
                row_id = 'row-' + spec['key'].replace('.', '-')
                with Horizontal(classes='field-row', id=row_id):
                    yield Label(spec['label'], classes='field-label')
                    kind = spec['kind']
                    key = spec['key']
                    wid = 'f-' + key.replace('.', '-')
                    cur = self._field_value(key)
                    if kind == 'enum':
                        opts = [(v, v) for v in spec['values']]
                        opts.insert(0, ('(未设置)', UNSET))
                        value = cur if cur in spec['values'] else UNSET
                        self._initial_values[key] = value
                        yield Select(opts, value=value, id=wid, allow_blank=False)
                    elif kind == 'bool':
                        opts = [('(未设置)', UNSET), ('是', 'true'), ('否', 'false')]
                        if cur is True:
                            value = 'true'
                        elif cur is False:
                            value = 'false'
                        else:
                            value = UNSET
                        self._initial_values[key] = value
                        yield Select(opts, value=value, id=wid, allow_blank=False)
                    elif kind == 'scripts':
                        n = len(cur or [])
                        yield Button(f'{n} 步  ·  点此编辑', id='btn-scripts')
                    elif kind == 'forwards':
                        n = len(cur or [])
                        yield Button(f'{n} 条  ·  点此编辑', id='btn-forwards')
                    elif kind == 'password':
                        hint = spec.get('hint') or ''
                        if self.profile.get('has_password'):
                            hint = '已保存，留空则不改'
                        yield Input(value='', password=True, placeholder=hint, id=wid)
                    else:
                        if kind == 'list':
                            shown = format_list(cur)
                        elif cur is None:
                            shown = ''
                        else:
                            shown = str(cur)
                        self._initial_values[key] = shown
                        yield Input(value=shown, placeholder=spec.get('hint') or '', id=wid)
        with Horizontal(id='form-btns'):
            yield Button('保存', id='save', variant='success')
            yield Button('取消', id='cancel')
        yield Footer()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == 'save':
            self.action_save()
        elif event.button.id == 'cancel':
            self.action_back()
        elif event.button.id == 'btn-scripts':
            path = self._field_path('options.scripts')
            steps = get_path(self.raw, path) or []
            def done(new_steps: list) -> None:
                set_path(self.raw, path, new_steps or None)
                self.query_one('#btn-scripts', Button).label = f'{len(new_steps)} 步  ·  点此编辑'
            self.app.push_screen(ScriptsScreen(steps, done))
        elif event.button.id == 'btn-forwards':
            path = self._field_path('options.forwardedPorts')
            fwds = get_path(self.raw, path) or []
            def done(new_fwds: list) -> None:
                set_path(self.raw, path, new_fwds or None)
                self.query_one('#btn-forwards', Button).label = f'{len(new_fwds)} 条  ·  点此编辑'
            self.app.push_screen(ForwardsScreen(fwds, done))

    def _read_fields(self) -> Optional[str]:
        for spec in FIELDS:
            kind = spec['kind']
            if kind in ('scripts', 'forwards'):
                continue
            wid = 'f-' + spec['key'].replace('.', '-')
            try:
                w = self.query_one('#' + wid)
            except Exception:
                continue
            path = self._field_path(spec['key'])
            if kind == 'enum':
                val = w.value
                if val != self._initial_values.get(spec['key']):
                    set_path(self.raw, path, None if val == UNSET else val)
            elif kind == 'bool':
                val = w.value
                if val != self._initial_values.get(spec['key']):
                    if val == UNSET:
                        set_path(self.raw, path, None)
                    else:
                        set_path(self.raw, path, val == 'true')
            else:
                raw_text = (w.value if isinstance(w, Input) else '') or ''
                if raw_text == self._initial_values.get(spec['key']):
                    continue
                if kind == 'password':
                    if raw_text != '':
                        set_path(self.raw, path, raw_text)
                    # 留空：不改动，Lua 会保留原来的 password
                    continue

                text = raw_text.strip()
                if text == '':
                    set_path(self.raw, path, None)
                elif kind == 'list':
                    try:
                        set_path(self.raw, path, parse_list(raw_text))
                    except ValueError as exc:
                        return f"{spec['label']} {exc}"
                elif kind == 'number':
                    try:
                        number = int(text) if text.isdigit() or text.lstrip('-').isdigit() else float(text)
                        set_path(self.raw, path, number)
                    except ValueError:
                        return spec['label'] + ' 需要是数字'
                else:
                    set_path(self.raw, path, text)
        if not self.raw.get('name'):
            host = self._field_value('options.host')
            if host:
                self.raw['name'] = str(host)
        if not self._field_value('options.host'):
            return '主机不能为空'
        return None

    def action_save(self) -> None:
        err = self._read_fields()
        if err:
            self.notify(err, severity='error')
            return
        emit({'op': 'upsert', 'id': self.original_id, 'raw': self.raw})
        self.app.pop_screen()

    def action_back(self) -> None:
        self.app.pop_screen()


# ---------------------------------------------------------------------------
# main app
# ---------------------------------------------------------------------------


class SshManagerApp(App):
    TITLE = 'SSH Manager'
    CSS = """
    Screen {
        background: #1e1e2e;
        color: #cdd6f4;
    }
    #body { height: 1fr; }
    #left {
        width: 28;
        border: tall #89b4fa;
        background: #181825;
    }
    #right {
        width: 1fr;
        border: tall #6c7086;
        background: #1e1e2e;
    }
    #groups { height: 1fr; }
    #hosts { height: 1fr; }
    #filter { dock: top; margin: 0 0 1 0; }
    #hint { color: #a6adc8; padding: 0 1; height: 1; }
    #pane-title { color: #89b4fa; text-style: bold; padding: 0 1; height: 1; }
    ListView > ListItem { padding: 0 1; }
    ListView > ListItem.--highlight { background: #313244; }
    DataTable { height: 1fr; }
    #modal {
        width: 60;
        height: auto;
        padding: 1 2;
        background: #313244;
        border: tall #89b4fa;
        margin: 4 8;
    }
    #modal-btns { height: 3; align: left middle; }
    #form { height: 1fr; padding: 0 1; }
    .field-row { height: 3; }
    .field-label { width: 16; content-align: left middle; color: #a6adc8; }
    #form-btns { height: 3; padding: 0 1; }
    #form-btns Button { margin-right: 1; }
    """
    BINDINGS = [
        Binding('enter', 'connect', '连接', show=True),
        Binding('space', 'connect', '连接', show=False),
        Binding('ctrl+enter', 'connect_window', '新窗口', show=True),
        Binding('e', 'edit', '编辑'),
        Binding('n', 'new', '新建'),
        Binding('d', 'delete', '删除'),
        Binding('p', 'quick', '快捷连接'),
        Binding('r', 'reload', '刷新'),
        Binding('slash', 'focus_filter', '过滤', key_display='/'),
        Binding('q', 'hide', '返回'),
        Binding('escape', 'hide', show=False),
    ]

    def __init__(self, snapshot_path: str) -> None:
        super().__init__()
        self.snapshot_path = snapshot_path
        self.data: dict = {'profiles': [], 'groups': []}
        self.mtime: Optional[float] = None
        self.group: str = ALL
        self.filter_text: str = ''
        self._host_ids: list[str] = []

    def compose(self) -> ComposeResult:
        yield Header(show_clock=False)
        with Horizontal(id='body'):
            with Vertical(id='left'):
                yield Static('分组', id='pane-title')
                yield ListView(id='groups')
            with Vertical(id='right'):
                yield Input(placeholder='过滤名称 / 主机 / 用户  ·  / 聚焦', id='filter')
                yield DataTable(id='hosts', cursor_type='row', zebra_stripes=True)
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one('#hosts', DataTable)
        table.add_columns('名称', '目标', '')
        self.reload_snapshot(force=True)
        self.set_interval(0.4, self._poll)
        try:
            self.query_one('#hosts', DataTable).focus()
        except Exception:
            pass

    def _poll(self) -> None:
        self.reload_snapshot(force=False)

    def reload_snapshot(self, force: bool = False) -> None:
        path = self.snapshot_path
        try:
            mtime = os.path.getmtime(path)
        except OSError:
            if force:
                self.notify(f'读不到 snapshot：{path}', severity='error')
            return
        if not force and self.mtime is not None and mtime <= self.mtime:
            return
        try:
            self.data = _load(path)
            self.mtime = mtime
        except (OSError, json.JSONDecodeError):
            return
        try:
            self.refresh_groups()
            self.refresh_hosts()
        except Exception as exc:
            self.notify(f'刷新列表失败：{exc}', severity='error')

    def profiles(self) -> list[dict]:
        return [p for p in self.data.get('profiles') or [] if isinstance(p, dict)]

    def grouped(self) -> list[dict]:
        rows = []
        for p in self.profiles():
            if self.group != ALL and (p.get('group') or '') != self.group:
                continue
            if not _matches(p, self.filter_text):
                continue
            rows.append(p)
        return rows

    def refresh_groups(self) -> None:
        lv = self.query_one('#groups', ListView)
        counts: dict[str, int] = {}
        for p in self.profiles():
            g = p.get('group') or ''
            counts[g] = counts.get(g, 0) + 1
        total = sum(counts.values())
        names = [g for g in (self.data.get('groups') or []) if g]
        for g in counts:
            if g and g not in names:
                names.append(g)
        index = {ALL: 0}
        keep = self.group
        names_list = list(names)
        self._group_order = [ALL] + names_list
        lv.clear()
        lv.append(ListItem(Label(f'全部    {total}')))
        i = 1
        for g in names_list:
            n = counts.get(g, 0)
            index[g] = i
            lv.append(ListItem(Label(f'{g}    {n}')))
            i += 1
        target = 0 if keep == ALL else index.get(keep, 0)
        try:
            lv.index = target
        except Exception:
            pass

    def refresh_hosts(self) -> None:
        table = self.query_one('#hosts', DataTable)
        current = self.current_id()
        table.clear()
        self._host_ids = []
        for p in self.grouped():
            pid = str(p.get('id') or '')
            flag = ''
            if not p.get('editable', True):
                flag = '只读'
            elif p.get('jumpHost'):
                flag = 'via ' + str(p.get('jumpHost'))
            name = p.get('name') or p.get('id') or '?'
            if p.get('group') and self.group == ALL:
                name = f"{p.get('group')}/{name}"
            table.add_row(name, _target(p), flag, key=pid or None)
            self._host_ids.append(pid)
        if current:
            try:
                idx = self._host_ids.index(current)
                table.move_cursor(row=idx)
            except Exception:
                pass

    def current_id(self) -> Optional[str]:
        ids = getattr(self, '_host_ids', [])
        if not ids:
            return None
        table = self.query_one('#hosts', DataTable)
        row = getattr(table, 'cursor_row', None)
        if isinstance(row, int) and 0 <= row < len(ids) and ids[row]:
            return ids[row]
        try:
            rk = table.coordinate_to_cell_key(table.cursor_coordinate).row_key
            val = getattr(rk, 'value', rk)
            s = str(val)
            if s and s != 'None':
                return s
        except Exception:
            pass
        return None

    def current_profile(self) -> Optional[dict]:
        pid = self.current_id()
        if not pid:
            return None
        for p in self.profiles():
            if str(p.get('id')) == pid:
                return p
        return None

    def on_list_view_highlighted(self, event: ListView.Highlighted) -> None:
        if event.list_view.id != 'groups':
            return
        idx = event.list_view.index
        order = getattr(self, '_group_order', [ALL])
        if idx is None or idx < 0 or idx >= len(order):
            self.group = ALL
        else:
            self.group = order[idx]
        self.refresh_hosts()

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        if event.list_view.id == 'groups':
            self.query_one('#hosts', DataTable).focus()

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        if event.data_table.id != 'hosts':
            return
        self.action_connect()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == 'filter':
            self.filter_text = event.value or ''
            self.refresh_hosts()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == 'filter':
            self.query_one('#hosts', DataTable).focus()

    def _busy_input(self) -> bool:
        return isinstance(self.focused, Input)

    def _main_view_active(self) -> bool:
        return len(self.screen_stack) == 1

    def _hosts_focused(self) -> bool:
        return self._main_view_active() and getattr(self.focused, 'id', None) == 'hosts'

    def action_focus_filter(self) -> None:
        if not self._main_view_active():
            return
        self.query_one('#filter', Input).focus()

    def action_connect(self) -> None:
        if not self._hosts_focused():
            return
        p = self.current_profile()
        if not p:
            self.notify('先在右侧列表里选一台服务器，再按 Enter', severity='warning')
            try:
                self.query_one('#hosts', DataTable).focus()
            except Exception:
                pass
            return
        self.notify(f'正在连接 {p.get("id") or p.get("name")} …')
        emit({'op': 'connect', 'id': p['id'], 'where': self.data.get('default_where') or 'tab'})

    def action_connect_window(self) -> None:
        if not self._hosts_focused():
            return
        p = self.current_profile()
        if not p:
            self.notify('先在右侧列表里选一台服务器', severity='warning')
            return
        self.notify(f'正在新窗口连接 {p.get("id") or p.get("name")} …')
        emit({'op': 'connect', 'id': p['id'], 'where': 'window'})

    def action_edit(self) -> None:
        if not self._hosts_focused():
            return
        p = self.current_profile()
        if not p:
            return
        if not p.get('editable', True):
            def after(ok: bool) -> None:
                if ok:
                    emit({'op': 'copy_in', 'id': p['id']})
                    self.notify('已请求复制到可编辑 store')
            self.push_screen(
                ConfirmModal(f'「{p.get("name")}」只读（来自导入）。复制一份到 store 再编辑？'),
                after,
            )
            return
        self.push_screen(EditScreen(p, p.get('id')))

    def action_new(self) -> None:
        if not self._main_view_active() or self._busy_input():
            return
        def after(line: Optional[str]) -> None:
            if not line or not line.strip():
                return
            t = parse_target(line.strip())
            host = t.get('host') or line.strip()
            raw = {
                'name': host,
                'group': '' if self.group == ALL else self.group,
                'options': {'host': host},
            }
            if t.get('user'):
                raw['options']['user'] = t['user']
            if t.get('port'):
                raw['options']['port'] = t['port']
            fake = {
                'id': None,
                'name': host,
                'group': raw['group'],
                'host': host,
                'user': t.get('user'),
                'port': t.get('port'),
                'editable': True,
                'raw': raw,
            }
            self.push_screen(EditScreen(fake, None))
        self.push_screen(PromptModal('新连接：[user@]host[:port]', placeholder='ops@203.0.113.11'), after)

    def action_delete(self) -> None:
        if not self._hosts_focused():
            return
        p = self.current_profile()
        if not p:
            return
        if not p.get('editable', True):
            self.notify('只读连接不能删', severity='warning')
            return
        def after(ok: bool) -> None:
            if ok:
                emit({'op': 'delete', 'id': p['id']})
        self.push_screen(ConfirmModal(f'删除「{p.get("name")}」？'), after)

    def action_quick(self) -> None:
        if not self._main_view_active() or self._busy_input():
            return
        def after(line: Optional[str]) -> None:
            if not line or not line.strip():
                return
            emit({
                'op': 'quick',
                'target': line.strip(),
                'where': self.data.get('default_where') or 'tab',
            })
        self.push_screen(PromptModal('快捷连接：[user@]host[:port]', placeholder='user@host:22'), after)

    def action_reload(self) -> None:
        if not self._main_view_active():
            return
        emit({'op': 'reload'})
        self.notify('已请求刷新')

    def action_hide(self) -> None:
        if not self._main_view_active():
            return
        emit({'op': 'hide'})
        if not os.environ.get('WEZTERM_PANE'):
            self.exit()
