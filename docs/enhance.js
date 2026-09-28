/* Cinematic editorial — optional progressive enhancement.

   Everything here is additive: with scripting off, a page using this system is
   complete and every link still works. Load with `defer`.

   Two rules learned from shipping this system:
   1. Never put a user-visible string in this file. Read it from the document
      (a data attribute or existing text) so an Arabic page stays Arabic.
   2. Never fetch a web font or remote asset here. Surfaces that must work
      offline cannot depend on the network for their own appearance. */
(function () {
  'use strict';

  /* --- Platform-aware recommendation ---------------------------------
     Marks the row matching the visitor's platform. It never hides or reorders
     the others, and the label text comes from the element itself. */
  var uaAll = ((navigator.userAgentData && navigator.userAgentData.platform) ||
    navigator.platform || '') + ' ' + navigator.userAgent;

  var platform = null;
  if (/arch/i.test(uaAll)) platform = 'arch';
  else if (/debian|ubuntu|linux/i.test(uaAll)) platform = 'debian';
  else if (/win/i.test(uaAll)) platform = 'windows';

  if (platform) {
    var row = document.querySelector('[data-platform="' + platform + '"]');
    if (row) {
      row.setAttribute('data-primary', 'true');

      // The prefix is per-row and per-language, so every candidate row carries
      // its own — not just the one the developer happened to test.
      var badge = row.querySelector('[data-prefix]');
      if (badge) badge.textContent = badge.getAttribute('data-prefix') + badge.textContent;

      var note = document.querySelector('[data-detected]');
      if (note) note.textContent = note.getAttribute('data-detected');
    }
  }

  /* --- Copy controls ------------------------------------------------
     Added only when the Clipboard API is actually usable. Labels come from the
     document root so the control speaks the page's language. */
  if (navigator.clipboard && navigator.clipboard.writeText) {
    var label = function (name, fallback) {
      return document.documentElement.getAttribute('data-' + name) || fallback;
    };
    var labelCopy = label('label-copy', 'Copy');

    Array.prototype.forEach.call(document.querySelectorAll('.code'), function (block) {
      var head = block.querySelector('.code-head');
      var pre = block.querySelector('pre');
      if (!head || !pre) return;

      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'copy';
      btn.textContent = labelCopy;
      btn.setAttribute('aria-label', label('label-copy-aria', 'Copy this text'));

      btn.addEventListener('click', function () {
        navigator.clipboard.writeText(pre.innerText.trim()).then(function () {
          btn.textContent = label('label-copied', 'Copied');
          setTimeout(function () { btn.textContent = labelCopy; }, 1600);
        }).catch(function () {
          // Clipboard can fail without a user gesture or a secure context. Tell
          // the reader what to do instead of pretending it worked.
          btn.textContent = label('label-copy-manual', 'Press Ctrl+C');
          setTimeout(function () { btn.textContent = labelCopy; }, 2200);
        });
      });

      head.appendChild(btn);
    });
  }

  /* --- Contents: mobile disclosure + current section ----------------- */
  var toc = document.querySelector('.toc');
  var toggle = document.querySelector('.toc-toggle');
  if (toc && toggle) {
    toggle.addEventListener('click', function () {
      var open = toc.getAttribute('data-open') === 'true';
      toc.setAttribute('data-open', String(!open));
      toggle.setAttribute('aria-expanded', String(!open));
    });
  }

  var links = Array.prototype.slice.call(document.querySelectorAll('.toc a[href^="#"]'));
  var sections = links.map(function (a) {
    return document.getElementById(a.getAttribute('href').slice(1));
  }).filter(Boolean);

  if (!sections.length || !('IntersectionObserver' in window)) return;

  var visible = new Map();
  var observer = new IntersectionObserver(function (entries) {
    entries.forEach(function (e) { visible.set(e.target.id, e.isIntersecting); });
    for (var i = 0; i < sections.length; i++) {
      if (visible.get(sections[i].id)) {
        links.forEach(function (a) {
          if (a.getAttribute('href') === '#' + sections[i].id) a.setAttribute('aria-current', 'true');
          else a.removeAttribute('aria-current');
        });
        return;
      }
    }
  }, { rootMargin: '-76px 0px -60% 0px', threshold: 0 });

  sections.forEach(function (s) { observer.observe(s); });
})();
