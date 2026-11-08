// ONECOIN economy UI localization.
//
// Loads the bilingual message catalog from the node backend
// (GET /api/i18n/messages?locale=en|hy), caches it in localStorage and
// exposes window.AWEI18N.t(key, args) for every economy screen. The catalog
// itself lives in core/src/i18n.rs so node and desktop share one source.

(function () {
  'use strict';

  var LS_KEY = 'awenet.locale';
  var readyPromise = null;
  var locale = null;
  var messages = {};

  function readSavedLocale() {
    try {
      var saved = window.localStorage.getItem(LS_KEY);
      if (saved === 'en' || saved === 'hy') return saved;
    } catch (err) {
      // localStorage unavailable: default below.
    }
    return 'en';
  }

  function persistLocale(code) {
    try {
      window.localStorage.setItem(LS_KEY, code);
    } catch (err) {
      // Locale persistence is best-effort only.
    }
  }

  function apiBase() {
    if (window.AWEUI && typeof window.AWEUI.apiBase === 'function') {
      return window.AWEUI.apiBase();
    }
    return '';
  }

  function fetchCatalog(code) {
    return fetch(apiBase() + '/api/i18n/messages?locale=' + encodeURIComponent(code), {
      headers: { Accept: 'application/json' }
    })
      .then(function (res) {
        if (!res.ok) {
          throw new Error('i18n HTTP ' + res.status);
        }
        return res.json();
      })
      .then(function (body) {
        var map = body && body.messages && typeof body.messages === 'object'
          ? body.messages
          : {};
        if (code !== 'en') {
          // Merge over English so keys missing from a partial translation
          // fall back to English instead of raw identifiers.
          var merged = {};
          for (var k in map) merged[k] = map[k];
          messages = merged;
        } else {
          messages = map;
        }
        locale = code;
        return messages;
      });
  }

  function replaceArgs(text, args) {
    if (!args) return text;
    for (var name in args) {
      if (!Object.prototype.hasOwnProperty.call(args, name)) continue;
      var placeholder = '{' + name + '}';
      if (text.indexOf(placeholder) !== -1) {
        text = text.split(placeholder).join(String(args[name]));
      }
    }
    return text;
  }

  var api = {
    // Resolves once the catalog for the active locale is loaded. Safe to
    // await from every screen before its first render.
    ready: function () {
      if (!readyPromise) {
        readyPromise = fetchCatalog(readSavedLocale());
      }
      return readyPromise;
    },

    locale: function () {
      return locale || readSavedLocale();
    },

    t: function (key, args) {
      var text = Object.prototype.hasOwnProperty.call(messages, key)
        ? messages[key]
        : key;
      return replaceArgs(text, args);
    },

    // Switches locale, persists the choice and re-renders the active screen
    // by re-clicking the current nav entry.
    setLocale: function (code) {
      if (code !== 'en' && code !== 'hy') code = 'en';
      if (code === locale) return Promise.resolve(locale);
      persistLocale(code);
      readyPromise = fetchCatalog(code);
      return readyPromise.then(function () {
        document.documentElement.lang = code === 'hy' ? 'hy' : 'en';
        refreshChrome();
        refreshActiveScreen();
        return locale;
      });
    }
  };

  function refreshChrome() {
    var btn = document.getElementById('langToggle');
    if (btn) {
      btn.textContent = locale === 'hy' ? 'EN' : 'ՀԱ';
      btn.title = locale === 'hy' ? 'English' : 'Հայերեն';
    }
  }

  function refreshActiveScreen() {
    var active = document.querySelector('#nav a.active, nav a.active');
    if (active && typeof active.click === 'function') {
      active.click();
    }
  }

  window.AWEI18N = api;

  // The topbar language toggle: scripts run at the end of <body>, so the
  // button is already in the DOM at this point.
  var toggle = document.getElementById('langToggle');
  if (toggle) {
    toggle.addEventListener('click', function () {
      api.setLocale(locale === 'hy' ? 'en' : 'hy');
    });
  }
})();
