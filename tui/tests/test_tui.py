import asyncio
import json

from textual.widgets import Input

import sshmgr_tui.app as app_module
from sshmgr_tui.app import EditScreen, PromptModal, ScriptsScreen, SshManagerApp
from sshmgr_tui.fields import format_list, parse_list, resolve_field_path


def _run(coro):
    return asyncio.run(coro)


def _snapshot(tmp_path, raw=None):
    raw = raw or {'name': 'box', 'options': {'host': '192.0.2.1'}}
    path = tmp_path / 'snapshot.json'
    path.write_text(
        json.dumps(
            {
                'profiles': [
                    {
                        'id': 'box',
                        'name': raw.get('name', 'box'),
                        'group': raw.get('group', ''),
                        'host': raw.get('host') or raw.get('options', {}).get('host'),
                        'user': raw.get('user') or raw.get('options', {}).get('user'),
                        'port': raw.get('port') or raw.get('options', {}).get('port') or 22,
                        'editable': True,
                        'raw': raw,
                    }
                ],
                'groups': [],
                'default_where': 'tab',
            }
        ),
        encoding='utf-8',
    )
    return path


def test_raw_field_paths_and_lists_are_lossless():
    flat = {'host': 'server', 'private_keys': ['id,a', 'id-b']}
    nested = {'options': {'host': 'server', 'private_keys': ['id-a']}}

    assert resolve_field_path(flat, 'options.host') == 'host'
    assert resolve_field_path(flat, 'options.privateKeys') == 'private_keys'
    assert resolve_field_path(flat, 'options.password') == 'password'
    assert resolve_field_path(nested, 'options.host') == 'options.host'
    assert resolve_field_path(nested, 'options.privateKeys') == 'options.private_keys'
    assert resolve_field_path(nested, 'options.password') == 'options.password'
    assert (
        resolve_field_path(
            {'privateKeys': ['effective'], 'options': {'private_keys': ['ignored']}},
            'options.privateKeys',
        )
        == 'privateKeys'
    )

    values = ['printf "a,b"', 'echo c']
    assert parse_list(format_list(values)) == values
    assert parse_list('printf "a,b"') == ['printf "a,b"']


def test_editing_flat_profile_preserves_lists_and_password_whitespace(tmp_path):
    raw = {
        'name': 'flat',
        'host': '192.0.2.8',
        'user': 'ops',
        'private_keys': ['key,with-comma', 'key-two'],
        'on_login': ['printf "a,b"', 'echo done'],
    }
    profile = {
        'id': 'flat',
        'name': 'flat',
        'host': raw['host'],
        'user': raw['user'],
        'port': 22,
        'editable': True,
        'raw': raw,
    }
    app = SshManagerApp(str(_snapshot(tmp_path, raw)))

    async def exercise():
        async with app.run_test() as pilot:
            screen = EditScreen(profile, 'flat')
            app.push_screen(screen)
            await pilot.pause()

            assert screen.query_one('#f-options-host', Input).value == '192.0.2.8'
            screen.query_one('#f-name', Input).value = 'renamed'
            screen.query_one('#f-options-password', Input).value = '  secret  '
            assert screen._read_fields() is None

            assert screen.raw['name'] == 'renamed'
            assert screen.raw['host'] == '192.0.2.8'
            assert screen.raw['private_keys'] == ['key,with-comma', 'key-two']
            assert screen.raw['on_login'] == ['printf "a,b"', 'echo done']
            assert screen.raw['password'] == '  secret  '
            assert 'options' not in screen.raw

    _run(exercise())


def test_connect_keys_stay_on_host_list_and_do_not_capture_prompt(tmp_path, monkeypatch):
    sent = []
    monkeypatch.setattr(app_module, 'emit', sent.append)
    app = SshManagerApp(str(_snapshot(tmp_path)))

    async def exercise():
        async with app.run_test() as pilot:
            await pilot.pause()
            assert app.focused.id == 'hosts'

            await pilot.press('enter')
            await pilot.press('space')
            await pilot.press('ctrl+enter')
            assert [message['where'] for message in sent] == ['tab', 'tab', 'window']

            await pilot.press('n')
            await pilot.pause()
            assert isinstance(app.screen, PromptModal)
            prompt = app.screen.query_one('#prompt', Input)
            prompt.value = 'new.example'

            await pilot.press('space')
            await pilot.press('ctrl+enter')
            assert prompt.value.endswith(' ')
            assert len(sent) == 3

            prompt.value = 'new.example'
            await pilot.press('enter')
            await pilot.pause()
            assert isinstance(app.screen, EditScreen)
            assert len(sent) == 3

    _run(exercise())


def test_repeated_group_click_does_not_connect(tmp_path, monkeypatch):
    sent = []
    monkeypatch.setattr(app_module, 'emit', sent.append)
    app = SshManagerApp(str(_snapshot(tmp_path)))

    async def exercise():
        async with app.run_test() as pilot:
            await pilot.pause()
            await pilot.click('#groups', offset=(2, 1))
            await pilot.click('#groups', offset=(2, 1))
            assert sent == []

    _run(exercise())


def test_canceling_script_prompts_does_not_rewrite_a_string_step(tmp_path):
    app = SshManagerApp(str(_snapshot(tmp_path)))

    async def exercise():
        async with app.run_test() as pilot:
            screen = ScriptsScreen(['echo a,b'], lambda _steps: None)
            app.push_screen(screen)
            await pilot.pause()

            screen._edit(0)
            await pilot.pause()
            assert isinstance(app.screen, PromptModal)
            await pilot.press('escape')
            await pilot.pause()
            assert app.screen is screen
            assert screen.steps == ['echo a,b']

            screen._edit(0)
            await pilot.pause()
            app.screen.query_one('#prompt', Input).value = 'shell prompt'
            await pilot.press('enter')
            await pilot.pause()
            assert isinstance(app.screen, PromptModal)
            await pilot.press('escape')
            await pilot.pause()
            assert app.screen is screen
            assert screen.steps == ['echo a,b']

    _run(exercise())
