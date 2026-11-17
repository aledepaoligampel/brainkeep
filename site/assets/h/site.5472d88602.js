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

  // Manual install: macOS / Linux switch. The copy button next to it follows the visible block.
  Array.prototype.forEach.call(document.querySelectorAll('[data-os]'), function (tab) {
    tab.addEventListener('click', function () {
      var card = tab.closest('.manual-panes');
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

  // Hero screenshot: tilted back while the page is at the top, flat once half of it is in view
  // (or after half a screen of scroll, whichever is later).
  var shot = document.querySelector('.hero-shot');
  var still = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (shot && !still) {
    var flatAt = 1;
    var queued = false;
    var measure = function () {
      // Layout offsets ignore the transform, so the tilt never changes its own measurement.
      var top = 0;
      for (var el = shot; el; el = el.offsetParent) top += el.offsetTop;
      // At least half a screen of scroll, so the tilt eases out gradually even when it starts mostly in view.
      flatAt = Math.max(window.innerHeight / 2, top + shot.offsetHeight / 2 - window.innerHeight);
    };
    var update = function () {
      queued = false;
      var p = Math.min(1, Math.max(0, window.scrollY / flatAt));
      var eased = 1 - p * p * (3 - 2 * p);
      shot.style.setProperty('--tilt', eased.toFixed(3));
    };
    var queue = function () {
      if (!queued) { queued = true; window.requestAnimationFrame(update); }
    };
    measure();
    update();
    window.addEventListener('scroll', queue, { passive: true });
    window.addEventListener('resize', function () { measure(); queue(); });
    window.addEventListener('load', function () { measure(); queue(); });
  }
})();
