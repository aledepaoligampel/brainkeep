// Applies a saved theme before first paint. Without it the page follows the system theme.
(function () {
  try {
    var theme = window.localStorage.getItem('brainkeep.theme');
    if (theme === 'light' || theme === 'dark') document.documentElement.setAttribute('data-theme', theme);
  } catch (e) { /* storage unavailable */ }
})();
