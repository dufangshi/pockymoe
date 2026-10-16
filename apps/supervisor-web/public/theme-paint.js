// Paint the last theme before the app loads, not white and then the dark
// default. A blocking external script: the public site's CSP allows no inline
// script. src/app.tsx keeps `pockymoe.paint` up to date.
(function () {
  try {
    var root = document.documentElement;
    var paint = JSON.parse(localStorage.getItem('pockymoe.paint') || 'null') || {};
    var mode = localStorage.getItem('remote-codex-theme-mode');
    var effective = paint.fixed ? paint.effective
      : mode === 'light' || mode === 'dark' ? mode
      : matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    root.setAttribute('data-theme-effective', effective);
    root.style.colorScheme = effective;
    root.style.backgroundColor = paint.effective === effective && paint.background
      ? paint.background : effective === 'light' ? '#f4f6f3' : '#111416';
  } catch (error) { /* Storage blocked: the app paints on load. */ }
})();
