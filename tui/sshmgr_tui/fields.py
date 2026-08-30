"""Form field specs and lossless helpers for the profile editor.

Keep ``FIELDS`` in sync with ``plugin/sshmgr/panel.lua``.  Raw profiles are
allowed to use either the documented flat form or Tabby's nested ``options``
form, so the editor must not assume that every option lives below
``options``.
"""

import json

FIELDS = [
    {'key': 'name', 'label': '名称', 'kind': 'text'},
    {'key': 'group', 'label': '分组', 'kind': 'text', 'hint': '用 / 分隔层级'},
    {'key': 'options.host', 'label': '主机', 'kind': 'text'},
    {'key': 'options.port', 'label': '端口', 'kind': 'number'},
    {'key': 'options.user', 'label': '用户名', 'kind': 'text'},
    {
        'key': 'options.auth',
        'label': '认证方式',
        'kind': 'enum',
        'values': ['agent', 'publicKey', 'password', 'keyboardInteractive'],
    },
    {
        'key': 'options.password',
        'label': '密码',
        'kind': 'password',
        'hint': '明文写入配置文件；已有密码时留空不改',
    },
    {
        'key': 'options.privateKeys',
        'label': '私钥路径',
        'kind': 'list',
        'hint': '单项直接输入；多项用 JSON 数组',
    },
    {'key': 'options.password_env', 'label': '密码环境变量', 'kind': 'text'},
    {
        'key': 'options.jumpHost',
        'label': '跳板机',
        'kind': 'text',
        'hint': 'profile 名字，或 user@host:port',
    },
    {'key': 'options.agentForward', 'label': 'Agent 转发', 'kind': 'bool'},
    {'key': 'options.x11', 'label': 'X11 转发', 'kind': 'bool'},
    {'key': 'options.skipBanner', 'label': '跳过 banner', 'kind': 'bool'},
    {'key': 'options.keepaliveInterval', 'label': '保活间隔', 'kind': 'number', 'hint': '秒'},
    {'key': 'options.readyTimeout', 'label': '连接超时', 'kind': 'number', 'hint': '秒'},
    {'key': 'options.forwardedPorts', 'label': '端口转发', 'kind': 'forwards'},
    {'key': 'options.scripts', 'label': '登录脚本', 'kind': 'scripts'},
    {
        'key': 'on_login',
        'label': '登录后命令',
        'kind': 'list',
        'hint': '单条直接输入；多条用 JSON 数组',
    },
    {
        'key': 'behaviorOnSessionEnd',
        'label': '断开后行为',
        'kind': 'enum',
        'values': ['close', 'keep', 'reconnect'],
    },
    {'key': 'color', 'label': '颜色', 'kind': 'text', 'hint': '#rrggbb'},
    {'key': 'icon', 'label': '图标', 'kind': 'text', 'hint': '一个字符'},
]


# Canonical Tabby spelling -> accepted snake_case spelling.  These are the
# aliases that can appear in a hand-written raw profile (at either level).
OPTION_ALIASES = {
    'privateKeys': ('private_keys',),
    'jumpHost': ('jump_host',),
    'agentForward': ('agent_forward',),
    'skipBanner': ('skip_banner',),
    'keepaliveInterval': ('keepalive_interval',),
    'readyTimeout': ('ready_timeout',),
    'forwardedPorts': ('forwarded_ports',),
    'scripts': ('login_scripts',),
}


def get_path(obj, path):
    cur = obj
    for part in path.split('.'):
        if not isinstance(cur, dict):
            return None
        cur = cur.get(part)
    return cur


def set_path(obj, path, value):
    parts = path.split('.')
    cur = obj
    for part in parts[:-1]:
        nxt = cur.get(part)
        if not isinstance(nxt, dict):
            nxt = {}
            cur[part] = nxt
        cur = nxt
    if value is None:
        cur.pop(parts[-1], None)
    else:
        cur[parts[-1]] = value


def resolve_field_path(obj, path):
    """Return the actual raw-profile path backing a canonical form field.

    Existing keys win, including snake_case aliases.  When an option is not
    present yet, a profile without an ``options`` table is treated as flat so
    newly edited fields do not silently change its representation.
    """
    prefix = 'options.'
    if not path.startswith(prefix):
        return path

    key = path[len(prefix):]
    aliases = OPTION_ALIASES.get(key, ())
    options = obj.get('options')
    # Match profiles.normalize precedence: nested canonical, flat canonical,
    # nested alias, then flat alias.
    if isinstance(options, dict) and key in options:
        return path
    if key in obj:
        return key
    if isinstance(options, dict):
        for alias in aliases:
            if alias in options:
                return f'options.{alias}'
    for alias in aliases:
        if alias in obj:
            return alias

    # No existing key: retain the profile's overall flat/nested style.
    if not isinstance(options, dict):
        return key
    return path


def format_list(value):
    """Render a list without making commas inside an item ambiguous."""
    if isinstance(value, list):
        return json.dumps(value, ensure_ascii=False)
    if value is None:
        return ''
    return str(value)


def parse_list(text):
    """Parse a list field.

    A plain value means one item (and may itself contain commas).  Multiple
    items use an explicit JSON string array.
    """
    if text == '':
        return None
    if not text.lstrip().startswith('['):
        return [text]
    try:
        value = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ValueError('需要是 JSON 字符串数组') from exc
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ValueError('需要是 JSON 字符串数组')
    return value or None


def parse_target(spec: str) -> dict:
    spec = spec.strip()
    out = {}
    rest = spec
    if '@' in rest:
        user, rest = rest.split('@', 1)
        out['user'] = user
    if rest.startswith('[') and ']' in rest:
        inside, after = rest[1:].split(']', 1)
        out['host'] = inside
        if after.startswith(':'):
            try:
                out['port'] = int(after[1:])
            except ValueError:
                pass
        return out
    if rest.count(':') == 1:
        host, port = rest.rsplit(':', 1)
        if port.isdigit():
            out['host'] = host
            out['port'] = int(port)
            return out
    out['host'] = rest
    return out
