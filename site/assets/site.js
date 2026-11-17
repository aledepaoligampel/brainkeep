// brainkeep.dev: theme switch and copy button. The page works without this script.
(function () {
  var root = document.documentElement;

  function current() {
    var forced = root.getAttribute('data-theme');
    if (forced === 'light' || forced === 'dark') return forced;
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }

  var toggle = document.querySelector('.theme-toggle');
  if (toggle) {
    toggle.addEventListener('click', function () {
      var next = current() === 'dark' ? 'light' : 'dark';
      root.setAttribute('data-theme', next);
      try { window.localStorage.setItem('brainkeep.theme', next); } catch (e) { /* storage unavailable */ }
    });
  }

  // Quick start: macOS / Linux switch. The copy button follows the visible block.
  Array.prototype.forEach.call(document.querySelectorAll('[data-os]'), function (tab) {
    tab.addEventListener('click', function () {
      var card = tab.closest('.terminal');
      if (!card) return;
      Array.prototype.forEach.call(card.querySelectorAll('[data-os]'), function (other) {
        var active = other === tab;
        other.setAttribute('aria-pressed', active ? 'true' : 'false');
        var pane = document.getElementById(other.getAttribute('data-os'));
        if (pane) pane.hidden = !active;
      });
      var copy = card.querySelector('[data-copy]');
      if (copy) copy.setAttribute('data-copy', tab.getAttribute('data-os'));
    });
  });

  var status = document.getElementById('copy-status');
  Array.prototype.forEach.call(document.querySelectorAll('[data-copy]'), function (button) {
    var label = button.querySelector('.copy-text');
    var idle = label ? label.textContent : '';
    var timer = null;
    button.addEventListener('click', function () {
      var source = document.getElementById(button.getAttribute('data-copy'));
      if (!source || !navigator.clipboard) return;
      navigator.clipboard.writeText(source.textContent).then(function () {
        if (label) label.textContent = button.getAttribute('data-copied');
        if (status) status.textContent = button.getAttribute('data-status');
        clearTimeout(timer);
        timer = setTimeout(function () {
          if (label) label.textContent = idle;
          if (status) status.textContent = '';
        }, 1800);
      }, function () { /* clipboard denied: the commands stay selectable */ });
    });
  });
})();
