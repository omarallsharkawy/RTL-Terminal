(function () {
  var box = document.getElementById('err-overlay');
  var txt = document.getElementById('err-text');

  if (!box || !txt) return;

  function show(label, body) {
    box.style.display = 'block';
    box.dataset.reason = label;
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

  // Surface a genuinely stalled mount, but cancel the warning as soon as
  // React populates the root. Development builds can take longer than six
  // seconds on the first transform; a late successful mount must also clear a
  // timeout-only warning rather than covering the working terminal forever.
  var root = document.getElementById('root');
  var mountTimer = setTimeout(function () {
    if (root && root.children.length === 0) {
      show(
        'mount-timeout',
        'React did not mount within 6s.\n' +
          'Likely cause: an error during initial render (see above) or a build/runtime problem.'
      );
    }
  }, 6000);

  if (root) {
    var observer = new MutationObserver(function () {
      if (root.children.length === 0) return;

      clearTimeout(mountTimer);
      observer.disconnect();
      if (box.dataset.reason === 'mount-timeout') {
        box.style.display = 'none';
        box.removeAttribute('data-reason');
        txt.textContent = '';
      }
    });
    observer.observe(root, { childList: true });
  }
})();
