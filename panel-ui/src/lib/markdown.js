// Pure helpers for the Brainkeep panel. No DOM access: the output is a plain tree that the
// app turns into elements with textContent, so stored HTML can never execute.

const MAX_INPUT = 32768;

/** Returns the URL when it is an absolute http(s) link, otherwise null. */
export function safeHref(raw) {
  if (typeof raw !== 'string' || !/^https?:\/\/[^\s<>"'`\\]+$/i.test(raw)) return null;
  try {
    const url = new URL(raw);
    return (url.protocol === 'https:' || url.protocol === 'http:') && url.hostname ? raw : null;
  } catch {
    return null;
  }
}

function pushText(out, value) {
  if (!value) return;
  // Bare http(s) URLs become links; everything else stays text.
  const re = /https?:\/\/[^\s<>"'`\\)\]]+/gi;
  let last = 0;
  for (const match of value.matchAll(re)) {
    const href = safeHref(match[0]);
    if (!href) continue;
    if (match.index > last) out.push({ t: 'text', v: value.slice(last, match.index) });
    out.push({ t: 'a', href, children: [{ t: 'text', v: match[0] }] });
    last = match.index + match[0].length;
  }
  if (last < value.length) out.push({ t: 'text', v: value.slice(last) });
}

/** Inline subset: `code`, **strong**, *em*, [label](http(s) url). */
export function parseInline(src, depth = 0) {
  const out = [];
  let buffer = '';
  let i = 0;
  const flush = () => { pushText(out, buffer); buffer = ''; };
  while (i < src.length) {
    const c = src[i];
    if (c === '`') {
      const end = src.indexOf('`', i + 1);
      if (end > i) { flush(); out.push({ t: 'code', v: src.slice(i + 1, end) }); i = end + 1; continue; }
    } else if (c === '*' && src[i + 1] === '*' && depth < 3) {
      const end = src.indexOf('**', i + 2);
      if (end > i + 1) {
        flush();
        out.push({ t: 'strong', children: parseInline(src.slice(i + 2, end), depth + 1) });
        i = end + 2;
        continue;
      }
    } else if (c === '*' && depth < 3) {
      const end = src.indexOf('*', i + 1);
      if (end > i + 1 && src[i + 1] !== ' ') {
        flush();
        out.push({ t: 'em', children: parseInline(src.slice(i + 1, end), depth + 1) });
        i = end + 1;
        continue;
      }
    } else if (c === '[') {
      const mid = src.indexOf('](', i + 1);
      const end = mid > i ? src.indexOf(')', mid + 2) : -1;
      if (end > mid && !src.slice(i + 1, mid).includes('[')) {
        const href = safeHref(src.slice(mid + 2, end));
        if (href) {
          flush();
          out.push({ t: 'a', href, children: parseInline(src.slice(i + 1, mid), depth + 1) });
          i = end + 1;
          continue;
        }
      }
    }
    buffer += c;
    i += 1;
  }
  flush();
  return out;
}

function inlineLines(lines) {
  const children = [];
  lines.forEach((line, n) => {
    if (n > 0) children.push({ t: 'br' });
    children.push(...parseInline(line));
  });
  return children;
}

/** Block subset: headings, paragraphs, bullet/numbered lists and fenced code. */
export function parseMarkdown(source) {
  const lines = String(source ?? '').slice(0, MAX_INPUT).replace(/\r\n?/g, '\n').split('\n');
  const blocks = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      const body = [];
      i += 1;
      while (i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i++]);
      i += 1;
      blocks.push({ t: 'pre', v: body.join('\n') });
      continue;
    }
    const heading = /^(#{1,6})\s+([\s\S]*)$/.exec(line);
    if (heading) {
      blocks.push({ t: 'h', level: heading[1].length, children: parseInline(heading[2]) });
      i += 1;
      continue;
    }
    const bullet = /^\s*[-*+]\s+/;
    const number = /^\s*\d{1,9}[.)]\s+/;
    for (const [re, t] of [[bullet, 'ul'], [number, 'ol']]) {
      if (!re.test(line)) continue;
      const items = [];
      while (i < lines.length && re.test(lines[i])) {
        items.push({ t: 'li', children: parseInline(lines[i].replace(re, '')) });
        i += 1;
      }
      blocks.push({ t, children: items });
      break;
    }
    if (bullet.test(line) || number.test(line)) continue;
    if (!line.trim()) { i += 1; continue; }
    const para = [];
    while (i < lines.length && lines[i].trim() && !/^\s*```/.test(lines[i]) && !/^#{1,6}\s/.test(lines[i])
      && !bullet.test(lines[i]) && !number.test(lines[i])) para.push(lines[i++]);
    // Guarantee progress if a line matches no block rule above.
    if (!para.length) para.push(lines[i++]);
    blocks.push({ t: 'p', children: inlineLines(para) });
  }
  return blocks;
}

/** Line diff: common prefix/suffix, then LCS on the middle (bounded). */
export function lineDiff(before, after) {
  const a = String(before ?? '').split('\n');
  const b = String(after ?? '').split('\n');
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) { endA -= 1; endB -= 1; }
  const head = a.slice(0, start).map((t) => ({ op: ' ', t }));
  const tail = a.slice(endA).map((t) => ({ op: ' ', t }));
  const x = a.slice(start, endA);
  const y = b.slice(start, endB);
  const middle = [];
  if (x.length * y.length > 4_000_000) {
    x.forEach((t) => middle.push({ op: '-', t }));
    y.forEach((t) => middle.push({ op: '+', t }));
  } else {
    const n = x.length;
    const m = y.length;
    const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let p = n - 1; p >= 0; p -= 1) {
      for (let q = m - 1; q >= 0; q -= 1) {
        dp[p][q] = x[p] === y[q] ? dp[p + 1][q + 1] + 1 : Math.max(dp[p + 1][q], dp[p][q + 1]);
      }
    }
    let p = 0;
    let q = 0;
    while (p < n || q < m) {
      if (p < n && q < m && x[p] === y[q]) { middle.push({ op: ' ', t: x[p] }); p += 1; q += 1; }
      else if (q >= m || (p < n && dp[p + 1][q] >= dp[p][q + 1])) { middle.push({ op: '-', t: x[p] }); p += 1; }
      else { middle.push({ op: '+', t: y[q] }); q += 1; }
    }
  }
  return [...head, ...middle, ...tail];
}

/** Verified author of a revision; revisions saved before schema v3 have none. */
export function author(item) {
  return item.principal || '—';
}
