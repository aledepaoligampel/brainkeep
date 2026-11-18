"""PostToolUse hook that reminds Claude Code to save PCDD change milestones to Brainkeep."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HOOK = Path(__file__).resolve().parents[1]/'plugins/brainkeep/hooks/pcdd_reminder.py'

DOC = """# panel-v2

## Specs

- **S1 Panel.** Cita «x». Criterio: y.

## Tasks

- [x] T1 (S1) Approve scope
- [ ] T2 (S1) Build panel

## Evidence

No evidence yet.

## Continuation

Next step: build.

## Log

- L1 2026-11-17: «x».
"""


class Hook(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.state = self.root/'state'
        self.repo = self.root/'GitHub'/'My Project'
        (self.repo/'pcdd'/'changes').mkdir(parents=True)
        self.doc = self.repo/'pcdd'/'changes'/'panel-v2.md'
        self.doc.write_text(DOC)

    def tearDown(self):
        self.tmp.cleanup()

    def run_hook(self, payload, state=None):
        data = payload if isinstance(payload, str) else json.dumps(payload)
        env = dict(os.environ, CLAUDE_PLUGIN_DATA=str(state or self.state))
        p = subprocess.run([sys.executable, '-I', str(HOOK)], input=data, capture_output=True, text=True,
                           timeout=10, env=env)
        return p.returncode, p.stdout, p.stderr

    def edit(self, **kw):
        return self.run_hook({'hook_event_name': 'PostToolUse', 'tool_name': 'Edit',
                              'tool_input': {'file_path': str(self.doc)}}, **kw)

    def context(self, result):
        code, out, err = result
        self.assertEqual((code, err), (0, ''))
        body = json.loads(out)['hookSpecificOutput']
        self.assertEqual(body['hookEventName'], 'PostToolUse')
        return body['additionalContext']

    def test_first_edit_of_a_change_reminds(self):
        text = self.context(self.edit())
        self.assertIn('topic "panel-v2"', text)
        self.assertIn('project "my-project"', text)

    def test_edits_without_a_milestone_are_silent(self):
        self.context(self.edit())
        self.assertEqual(self.edit(), (0, '', ''))
        self.doc.write_text(DOC.replace('No evidence yet.', 'T1: tests pass.').replace('build.', 'review.'))
        self.assertEqual(self.edit(), (0, '', ''))

    def test_milestones_remind_once(self):
        self.context(self.edit())
        for old, new in (('- [ ] T2', '- [>] T2'),          # task state
                         ('Build panel', 'Build the panel'),  # task scope
                         ('Criterio: y.', 'Criterio: z.'),    # spec
                         ('«x».\n', '«x».\n- L2 2026-11-17: «si».\n')):  # approval in Log
            self.doc.write_text(self.doc.read_text().replace(old, new))
            self.assertIn('milestone', self.context(self.edit()), old)
            self.assertEqual(self.edit(), (0, '', ''), old)

    def test_shell_writes_to_change_documents_remind_on_milestones(self):
        cwd = str(self.repo)
        sed = {'tool_name': 'Bash', 'cwd': cwd,
               'tool_input': {'command': "sed -i '' 's/- \\[ \\] T2/- [x] T2/' pcdd/changes/panel-v2.md"}}
        text = self.context(self.run_hook(sed))
        self.assertIn('topic "panel-v2"', text)
        self.assertIn('project "my-project"', text)
        self.assertEqual(self.run_hook(sed), (0, '', ''))
        self.doc.write_text(self.doc.read_text().replace('- [ ] T2', '- [!] T2'))
        absolute = {'tool_name': 'Bash', 'cwd': '/elsewhere',
                    'tool_input': {'command': f"printf 'x' >> '{self.doc}'"}}
        self.assertIn('project "my-project"', self.context(self.run_hook(absolute)))

    def test_shell_mentions_without_milestones_are_silent(self):
        self.context(self.edit())
        command = 'echo hi > notes.md  # see pcdd/changes/panel-v2.md'
        self.assertEqual(self.run_hook({'tool_name': 'Bash', 'cwd': str(self.repo),
                                        'tool_input': {'command': command}}), (0, '', ''))

    def test_unusable_state_still_reminds(self):
        blocker = self.root/'not-a-dir'
        blocker.write_text('x')
        self.context(self.edit(state=blocker))
        self.context(self.edit(state=blocker))

    def test_other_files_and_bad_input_are_silent(self):
        for payload in ({'tool_input': {'file_path': '/r/src/main.rs'}},
                        {'tool_input': {'file_path': '/r/pcdd/changes/notes.txt'}},
                        {'tool_input': {'file_path': '/r/docs/pcdd/changes/x/y.md'}},
                        {'tool_input': {'file_path': '/r/pcdd/changes/missing.md'}},
                        {'tool_input': {'file_path': 'pcdd/changes/x.md'}},
                        {'tool_input': {}}, {'tool_input': 'x'}, [], 'not json', ''):
            self.assertEqual(self.run_hook(payload), (0, '', ''), payload)

    def test_shell_reads_and_other_files_are_silent(self):
        cwd = str(self.repo)
        for command in ('cat pcdd/changes/panel-v2.md', 'sed -n 1,20p pcdd/changes/panel-v2.md',
                        'grep -n Tasks pcdd/changes/panel-v2.md', 'echo hi > notes.md',
                        'echo x > pcdd/changes/missing.md'):
            self.assertEqual(self.run_hook({'tool_name': 'Bash', 'cwd': cwd, 'tool_input': {'command': command}}),
                             (0, '', ''), command)
        self.assertEqual(self.run_hook({'tool_name': 'Bash', 'tool_input': {'command': 'x > pcdd/changes/a.md'}}),
                         (0, '', ''))

    def test_artifact_publish_and_open_remind_to_save_link(self):
        for data in ({'file_path': '/r/page.html'}, {'action': 'open', 'url': 'https://claude.ai/artifact/x'}):
            text = self.context(self.run_hook({'tool_name': 'Artifact', 'cwd': '/Users/x/GitHub/pcdd',
                                               'tool_input': data}))
            self.assertIn('project "pcdd"', text)
        for data in ({'action': 'list'}, {'action': 'read', 'url': 'u'}, {'url': 'u', 'asset': True, 'file_path': 'a.png'}):
            self.assertEqual(self.run_hook({'tool_name': 'Artifact', 'cwd': '/r/pcdd', 'tool_input': data}),
                             (0, '', ''), data)


if __name__ == '__main__':
    unittest.main()
