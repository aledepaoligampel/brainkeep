"""PostToolUse hook that reminds Claude Code to save PCDD changes to Brainkeep."""
import json
import subprocess
import sys
import unittest
from pathlib import Path

HOOK = Path(__file__).resolve().parents[1]/'plugins/brainkeep/hooks/pcdd_reminder.py'


def run(payload):
    data = payload if isinstance(payload, str) else json.dumps(payload)
    p = subprocess.run([sys.executable, '-I', str(HOOK)], input=data, capture_output=True, text=True, timeout=10)
    return p.returncode, p.stdout, p.stderr


class Hook(unittest.TestCase):
    def test_change_document_edit_adds_context(self):
        code, out, err = run({'hook_event_name': 'PostToolUse', 'tool_name': 'Edit',
                              'tool_input': {'file_path': '/Users/x/GitHub/My Project/pcdd/changes/panel-v2.md'}})
        self.assertEqual((code, err), (0, ''))
        body = json.loads(out)['hookSpecificOutput']
        self.assertEqual(body['hookEventName'], 'PostToolUse')
        self.assertIn('topic "panel-v2"', body['additionalContext'])
        self.assertIn('project "my-project"', body['additionalContext'])

    def test_other_files_and_bad_input_are_silent(self):
        for payload in ({'tool_input': {'file_path': '/r/src/main.rs'}},
                        {'tool_input': {'file_path': '/r/pcdd/changes/notes.txt'}},
                        {'tool_input': {'file_path': '/r/docs/pcdd/changes/x/y.md'}},
                        {'tool_input': {'file_path': 'pcdd/changes/x.md'}},
                        {'tool_input': {}}, {'tool_input': 'x'}, [], 'not json', ''):
            self.assertEqual(run(payload), (0, '', ''), payload)

    def test_shell_writes_to_change_documents_add_context(self):
        for command in ("python3 - <<'EOF'\np='pcdd/changes/public-release.md'\nopen(p,'w').write(s)\nEOF",
                        "sed -i '' 's/- \\[ \\] T1/- [x] T1/' pcdd/changes/public-release.md",
                        "printf 'x' >> pcdd/changes/public-release.md"):
            code, out, err = run({'tool_name': 'Bash', 'cwd': '/Users/x/GitHub/pcdd', 'tool_input': {'command': command}})
            self.assertEqual((code, err), (0, ''))
            text = json.loads(out)['hookSpecificOutput']['additionalContext']
            self.assertIn('topic "public-release"', text)
            self.assertIn('project "pcdd"', text)

    def test_shell_reads_and_other_files_are_silent(self):
        for command in ('cat pcdd/changes/public-release.md', "sed -n 1,20p pcdd/changes/x.md",
                        'grep -n Tasks pcdd/changes/x.md', 'echo hi > notes.md'):
            self.assertEqual(run({'tool_name': 'Bash', 'cwd': '/r/pcdd', 'tool_input': {'command': command}}),
                             (0, '', ''), command)
        self.assertEqual(run({'tool_name': 'Bash', 'tool_input': {'command': 'x > pcdd/changes/a.md'}}), (0, '', ''))

    def test_artifact_publish_and_open_remind_to_save_link(self):
        for data in ({'file_path': '/r/page.html'}, {'action': 'open', 'url': 'https://claude.ai/artifact/x'}):
            code, out, _ = run({'tool_name': 'Artifact', 'cwd': '/Users/x/GitHub/pcdd', 'tool_input': data})
            self.assertIn('project "pcdd"', json.loads(out)['hookSpecificOutput']['additionalContext'])
        for data in ({'action': 'list'}, {'action': 'read', 'url': 'u'}, {'url': 'u', 'asset': True, 'file_path': 'a.png'}):
            self.assertEqual(run({'tool_name': 'Artifact', 'cwd': '/r/pcdd', 'tool_input': data}), (0, '', ''), data)


if __name__ == '__main__':
    unittest.main()
