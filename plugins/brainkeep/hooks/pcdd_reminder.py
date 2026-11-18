#!/usr/bin/env python3
"""Claude Code PostToolUse hook that reminds the model to record work in Brainkeep:
- when a PCDD change document (`<repo>/pcdd/changes/<name>.md`), edited with Edit/Write or
  by a shell command that writes it, reaches a milestone: its specs (S#), task lines (T#) or
  Log entries (L#) differ from the last time it reminded. Evidence and continuation edits
  stay silent. The last seen state lives in the plugin data directory; if it cannot be read
  or written, every edit reminds, as before;
- after an artifact is published or opened, so its link is saved for the team.
Silent (exit 0, no output) for anything else; never blocks the tool."""
import hashlib
import json
import os
import re
import sys
import tempfile
from pathlib import Path, PurePosixPath

NAME = r'pcdd/changes/[A-Za-z0-9._-]+\.md'
# Quoted paths may contain spaces; unquoted ones end at whitespace or shell punctuation.
CHANGE = re.compile(rf'''(?:'([^'\n]*{NAME})'|"([^"\n]*{NAME})"|((?:[^\s'"<>|;&]*/)?{NAME}))''')
# Shell commands that modify files; read-only commands (cat, grep, sed -n) stay silent.
WRITES = re.compile(r'>|\bsed\s+-i|\btee\b|\.write\(|write_text|\bperl\s+-i|\bmv\s|\bcp\s|\bpcdd\s+(new|time)\b')
# Lines whose change is a milestone: tasks (CLI syntax), specs and Log entries.
MILESTONE = re.compile(r'^\s*(?:[-*]\s+\[[ x>!]\]\s+T\d+|[-*]\s+(?:\*\*)?[SL]\d+\b|#{2,6}\s+S\d+\b)')
STATE_FILE = 'pcdd-milestones.json'


def slug(name):
    return re.sub(r'[^a-z0-9._-]+', '-', name.lower()).strip('-.')[:64]


def state_dir():
    base = os.environ.get('CLAUDE_PLUGIN_DATA')
    if base:
        return Path(base)
    cache = os.environ.get('XDG_CACHE_HOME') or str(Path.home()/'.cache')
    return Path(cache)/'brainkeep'


def milestone_reached(path):
    """True when the milestone lines changed since the last reminder (or state is unusable);
    False when unchanged; None when the document cannot be read."""
    try:
        lines = [line.strip() for line in path.read_text(encoding='utf-8').splitlines() if MILESTONE.match(line)]
    except (OSError, UnicodeDecodeError):
        return None
    digest = hashlib.sha256('\n'.join(lines).encode()).hexdigest()
    key, directory = str(path), state_dir()
    try:
        state = json.loads((directory/STATE_FILE).read_text(encoding='utf-8'))
        if not isinstance(state, dict):
            state = {}
    except FileNotFoundError:
        state = {}
    except (OSError, ValueError):
        return True
    if state.get(key) == digest:
        return False
    state[key] = digest
    try:
        directory.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=directory, prefix='.pcdd-')
        with os.fdopen(fd, 'w', encoding='utf-8') as out:
            json.dump(state, out)
        os.replace(tmp, directory/STATE_FILE)
    except OSError:
        pass
    return True


def change_text(path):
    change, project = path.name[:-3], slug(path.parts[-4])
    return (f'PCDD change "{change}" reached a milestone: its specs, task lines or Log entries changed '
            f'(pcdd/changes/{change}.md). Record the outcome in Brainkeep now: project "{project}", '
            f'topic "{change}", kind decision or progress, short and factual (recall and correct the '
            f'topic if it already exists). Do not narrate the save.')


def change_document(path):
    if not path.is_absolute() or len(path.parts) < 4 or path.parts[-3:-1] != ('pcdd', 'changes'):
        return None
    if not path.name[:-3] or not slug(path.parts[-4]):
        return None
    return path


def reminders(paths):
    texts = []
    for path in paths:
        if milestone_reached(path):
            texts.append(change_text(path))
    return ' '.join(texts) or None


def reminder(event):
    if not isinstance(event, dict):
        return None
    tool, data = event.get('tool_name'), event.get('tool_input')
    if not isinstance(data, dict):
        return None
    if tool == 'Bash':
        command, cwd = data.get('command'), event.get('cwd')
        if not isinstance(command, str) or not WRITES.search(command):
            return None
        paths = set()
        for groups in CHANGE.findall(command):
            found = next(g for g in groups if g)
            if not found.startswith('/'):
                if not isinstance(cwd, str) or not cwd.startswith('/'):
                    continue
                found = f'{cwd}/{found}'
            path = change_document(Path(os.path.normpath(found)))
            if path:
                paths.add(path)
        if not paths or len(paths) > 3:
            return None
        return reminders(sorted(paths))
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
    path = change_document(Path(path))
    return reminders([path]) if path else None


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
