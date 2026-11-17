// Applies a saved theme before first paint. Without it the page follows the system theme.
// Also marks the page as scripted, so effects driven by site.js start in their first state.
(function () {
  document.documentElement.classList.add('js');
  try {
    var theme = window.localStorage.getItem('brainkeep.theme');
    if (theme === 'light' || theme === 'dark') document.documentElement.setAttribute('data-theme', theme);
  } catch (e) { /* storage unavailable */ }
})();
