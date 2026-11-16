#!/usr/bin/env python3
"""Claude Code PostToolUse hook that reminds the model to record work in Brainkeep:
- after a change to a PCDD change document (`<repo>/pcdd/changes/<name>.md`), made with
  Edit/Write or by a shell command that writes it;
- after an artifact is published or opened, so its link is saved for the team.
Silent (exit 0, no output) for anything else; never blocks the tool."""
import json
import re
import sys
from pathlib import PurePosixPath

CHANGE = re.compile(r'pcdd/changes/([A-Za-z0-9._-]+)\.md')
# Shell commands that modify files; read-only commands (cat, grep, sed -n) stay silent.
WRITES = re.compile(r'>|\bsed\s+-i|\btee\b|\.write\(|write_text|\bperl\s+-i|\bmv\s|\bcp\s|\bpcdd\s+(new|time)\b')


def slug(name):
    return re.sub(r'[^a-z0-9._-]+', '-', name.lower()).strip('-.')[:64]


def change_text(change, project):
    return (f'PCDD change "{change}" was edited (pcdd/changes/{change}.md). If specs, task states, '
            f'decisions, approvals or evidence changed, record the outcome in Brainkeep now: '
            f'project "{project}", topic "{change}", kind decision or progress, short and factual '
            f'(recall and correct the topic if it already exists). Do not narrate the save.')


def reminder(event):
    if not isinstance(event, dict):
        return None
    tool, data = event.get('tool_name'), event.get('tool_input')
    if not isinstance(data, dict):
        return None
    if tool == 'Bash':
        command, cwd = data.get('command'), event.get('cwd')
        if not isinstance(command, str) or not isinstance(cwd, str) or not WRITES.search(command):
            return None
        changes = sorted(set(CHANGE.findall(command)))
        project = slug(PurePosixPath(cwd).name)
        if not changes or not project or len(changes) > 3:
            return None
        return ' '.join(change_text(c, project) for c in changes)
    if tool == 'Artifact':
        if data.get('action', 'publish') not in ('publish', 'open') or data.get('asset'):
            return None
        project = slug(PurePosixPath(event.get('cwd') or '').name) if isinstance(event.get('cwd'), str) else ''
        where = f'project "{project}"' if project else 'the related project'
        return ('An artifact was published or opened. If it is part of shared project work and its link '
                f'is not in Brainkeep yet, save it now in {where}: URL, what it is, who created it '
                '(never URLs with tokens). Do not narrate the save.')
    path = data.get('file_path')
    if not isinstance(path, str) or not path.endswith('.md'):
        return None
    parts = PurePosixPath(path).parts
    if len(parts) < 4 or parts[-3:-1] != ('pcdd', 'changes'):
        return None
    change, project = parts[-1][:-3], slug(parts[-4])
    if not change or not project:
        return None
    return change_text(change, project)


def main():
    try:
        text = reminder(json.load(sys.stdin))
    except Exception:
        return 0
    if text:
        json.dump({'hookSpecificOutput': {'hookEventName': 'PostToolUse', 'additionalContext': text}},
                  sys.stdout)
    return 0


if __name__ == '__main__':
    sys.exit(main())
