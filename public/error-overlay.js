(function () {
  var box = document.getElementById('err-overlay');
  var txt = document.getElementById('err-text');

  if (!box || !txt) return;

  function show(label, body) {
    box.style.display = 'block';
    txt.textContent += '==== ' + label + ' ====\n' + body + '\n\n';
  }

  window.addEventListener('error', function (e) {
    show(
      'error @ ' + (e.filename || '?') + ':' + e.lineno,
      e.message + (e.error && e.error.stack ? '\n' + e.error.stack : '')
    );
  });

  window.addEventListener('unhandledrejection', function (e) {
    var reason = e.reason;
    show(
      'unhandledrejection',
      (reason && reason.message ? reason.message : String(reason)) +
        (reason && reason.stack ? '\n' + reason.stack : '')
    );
  });

  // If React never populates #root after 6s, surface that explicitly.
  setTimeout(function () {
    var root = document.getElementById('root');
    if (root && root.children.length === 0) {
      show(
        'mount-timeout',
        'React did not mount within 6s.\n' +
          'Likely cause: an error during initial render (see above) or a build/runtime problem.'
      );
    }
  }, 6000);
})();
