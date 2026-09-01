/*
 * The development toolbar.
 *
 * Served from /_profiler/toolbar.js and appended to the end of every
 * server-rendered page. See docs/adr/0004-development-profiler.md, sections 6
 * and 7, for why this is vanilla JavaScript in a shadow root and not a Leptos
 * component. The two rules that follow from it:
 *
 *   1. Nothing here may touch the DOM before hydration has finished. Leptos
 *      hydrates <body>'s children by position; an element appended early
 *      shifts every index and takes the page. The application tells us when it
 *      is safe by calling window.__phonix_profiler.route(), which can only
 *      happen from an effect, which can only run after hydration.
 *
 *   2. Nothing here may read anything out of the application. A wasm panic
 *      freezes the whole reactive graph, and that is the moment the toolbar is
 *      most wanted. Both data paths point outwards: the app pushes its route
 *      in, and the toolbar reads its numbers from the profiler's own endpoint.
 */
(function () {
  "use strict";

  if (window.__phonix_profiler) {
    return;
  }

  var self = document.currentScript;

  var state = {
    // The document's own page id, minted by the server so that the navigation
    // and the server calls it goes on to make land in one group. Absent only
    // if this script was loaded some other way.
    page: (self && self.getAttribute("data-page")) || mint(),
    route: location.pathname,
    summary: null,
    error: null,
    mounted: false,
    open: false,
    // Deliberately not remembered across page loads. Closing it is "not now",
    // not "never again": a developer who dismissed it on Tuesday and reloads
    // on Wednesday has no way to know a tool exists that is switched off in a
    // storage key. A reload brings it back, and the handle brings it back
    // without one.
    hidden: false
  };

  var host = null;
  var shadow = null;
  var timer = null;

  /* --- the two ways in ---------------------------------------------------
   *
   * Both are called from outside this file: `route` by an effect in the
   * application, and the patched `fetch` by anything that makes a request.
   */

  window.__phonix_profiler = {
    /*
     * The application's current route, pushed one way.
     *
     * The first call is also the signal that hydration has finished, which is
     * what makes it safe to touch the DOM.
     */
    route: function (path) {
      mount();

      if (typeof path !== "string" || path === state.route) {
        return;
      }

      // An in-app navigation produces no document request, so nothing on the
      // server can mint this. A new screen is a new page load, and grouping it
      // with the last one would put two screens' server calls in one report.
      state.route = path;
      state.page = mint();
      state.summary = null;
      render();
      refresh();
    },

    /* Read by the report's own pages, and useful from a console. */
    id: function () {
      return state.page;
    }
  };

  var native = window.fetch;

  window.fetch = function (input, init) {
    var patched = init;

    try {
      var url = new URL(
        typeof input === "string" ? input : (input && input.url) || "",
        location.href
      );

      if (url.origin === location.origin && !isProfiler(url.pathname)) {
        patched = init ? assign({}, init) : {};
        var headers = new Headers(
          patched.headers ||
            (typeof input !== "string" && input && input.headers) ||
            {}
        );
        headers.set("X-Phonix-Page", state.page);
        patched.headers = headers;
      }
    } catch (error) {
      // A request the profiler cannot describe is still a request the
      // application is entitled to make. Nothing here may be the reason a
      // page fails.
      patched = init;
    }

    var result = native.call(this, input, patched);

    try {
      result.then(settled, settled);
    } catch (error) {
      /* not a promise; nothing to wait for */
    }

    return result;
  };

  function settled() {
    if (state.mounted) {
      refresh();
    }
  }

  /* --- mounting ----------------------------------------------------------- */

  /*
   * Draw the toolbar, once.
   *
   * Called from `route`, so on a healthy page this happens the moment the
   * application's first effect runs. The timer below is the other way in: if
   * the app never hydrates - a wasm panic, a build that half-finished - there
   * will be no call, and that is exactly the page somebody needs the toolbar
   * on. Eight seconds after load is late enough that a merely slow hydration
   * has finished and there is no cursor left to disturb.
   */
  function mount() {
    if (state.mounted) {
      return;
    }

    state.mounted = true;

    host = document.createElement("div");
    host.id = "phonix-profiler";
    // Inline, and important, because the page's own stylesheet is not ours to
    // predict and this element has to be where it says it is.
    host.style.setProperty("position", "fixed", "important");
    host.style.setProperty("left", "0", "important");
    host.style.setProperty("right", "0", "important");
    host.style.setProperty("bottom", "0", "important");
    host.style.setProperty("z-index", "2147483000", "important");
    // Never a width, and never 100vw: anything wider than the viewport
    // inflates it on a phone and throws every fixed overlay off-screen.
    host.style.setProperty("max-width", "100%", "important");

    shadow = host.attachShadow({ mode: "open" });
    shadow.innerHTML = "<style>" + STYLE + "</style><div id=\"root\"></div>";

    document.body.appendChild(host);

    render();
    refresh();
  }

  window.addEventListener("load", function () {
    setTimeout(mount, 8000);
  });

  /* --- data --------------------------------------------------------------- */

  function refresh() {
    if (timer) {
      clearTimeout(timer);
    }

    // Debounced, because a screen resolving eleven resources would otherwise
    // ask eleven times for the same answer.
    timer = setTimeout(function () {
      timer = null;

      native
        .call(window, "/_profiler/api/page/" + encodeURIComponent(state.page), {
          headers: { accept: "application/json" }
        })
        .then(function (response) {
          return response.ok ? response.json() : Promise.reject(response.status);
        })
        .then(function (summary) {
          state.summary = summary;
          state.error = null;
          render();
        })
        .catch(function () {
          // The commonest cause by a distance is the watcher restarting the
          // server, which drops every profile it held.
          state.error = "no profiles - did the server restart?";
          render();
        });
    }, 180);
  }

  /* --- drawing ------------------------------------------------------------ */

  function render() {
    if (!shadow) {
      return;
    }

    var root = shadow.getElementById("root");
    var summary = state.summary;

    if (state.hidden) {
      // In normal flow inside a right-aligned row, not absolutely positioned:
      // the host collapses to nothing when its only child is out of flow, and
      // a handle nobody can find is the same as no handle.
      root.innerHTML =
        "<div class=\"tray\"><button class=\"peek\" " +
        "title=\"show the profiler\">&#9889; profiler</button></div>";
      root.querySelector(".peek").onclick = function () {
        state.hidden = false;
        render();
      };
      return;
    }

    var slow = summary && summary.duration_ms >= 500;
    var repeated = (summary && summary.repeated) || [];

    var html =
      "<div class=\"bar" +
      (slow ? " slow" : "") +
      "\">" +
      "<button class=\"seg brand\" title=\"open this page load\">&#9889;</button>" +
      cell("route", escape(state.route)) +
      // "srv", not "time": this is server time summed across the group, and
      // the calls overlap. Calling it "time" would invite it to be read as
      // how long the screen took, which it is not.
      cell("srv", summary ? round(summary.duration_ms) + " ms" : "-") +
      cell(
        "req",
        summary ? String(summary.requests) : "-",
        summary && summary.errors ? "bad" : ""
      ) +
      cell(
        "sql",
        summary
          ? summary.queries + " / " + round(summary.sql_ms) + " ms"
          : "-"
      ) +
      (repeated.length
        ? "<span class=\"seg warn\" title=\"the same statement more than once\">" +
          "&#9888; " +
          repeated.length +
          " repeated</span>"
        : "") +
      (state.error ? "<span class=\"seg dim\">" + escape(state.error) + "</span>" : "") +
      "<span class=\"spacer\"></span>" +
      "<a class=\"seg\" href=\"/_profiler\" target=\"_blank\">report</a>" +
      "<button class=\"seg close\" title=\"hide\">&times;</button>" +
      "</div>";

    if (state.open) {
      html += panel(summary);
    }

    root.innerHTML = html;

    root.querySelector(".brand").onclick = function () {
      state.open = !state.open;
      render();
    };
    root.querySelector(".close").onclick = function () {
      state.hidden = true;
      state.open = false;
      render();
    };
  }

  function panel(summary) {
    if (!summary || !summary.profiles || !summary.profiles.length) {
      return (
        "<div class=\"panel\"><p class=\"dim\">Nothing recorded for this page load yet." +
        " An in-app navigation has no document request, so this fills in as the" +
        " screen asks for things.</p></div>"
      );
    }

    var rows = summary.profiles
      .map(function (profile) {
        return (
          "<tr class=\"" +
          (profile.status >= 400 ? "bad" : "") +
          "\">" +
          "<td>" +
          escape(profile.method) +
          "</td><td class=\"path\">" +
          escape(profile.route || profile.path) +
          "</td><td class=\"num\">" +
          profile.status +
          "</td><td class=\"num\">" +
          round(profile.duration) +
          " ms</td><td class=\"num\">" +
          profile.queries +
          "</td><td><a href=\"/_profiler/" +
          escape(profile.token) +
          "\" target=\"_blank\">open</a></td></tr>"
        );
      })
      .join("");

    return (
      "<div class=\"panel\">" +
      "<p class=\"dim\">" +
      summary.requests +
      " request(s) in this page load &middot; " +
      "<a href=\"/_profiler/page/" +
      escape(state.page) +
      "\" target=\"_blank\">full report</a></p>" +
      "<table><thead><tr><th></th><th>route</th><th class=\"num\">status</th>" +
      "<th class=\"num\">time</th><th class=\"num\">sql</th><th></th></tr></thead>" +
      "<tbody>" +
      rows +
      "</tbody></table></div>"
    );
  }

  function cell(label, value, extra) {
    return (
      "<span class=\"seg " +
      (extra || "") +
      "\"><i>" +
      label +
      "</i>" +
      value +
      "</span>"
    );
  }

  /* --- odds and ends ------------------------------------------------------ */

  function isProfiler(pathname) {
    return pathname.indexOf("/_profiler") === 0;
  }

  function mint() {
    try {
      if (window.crypto && crypto.randomUUID) {
        return crypto.randomUUID().replace(/-/g, "").slice(0, 16);
      }
    } catch (error) {
      /* fall through */
    }

    return (
      Date.now().toString(16) + Math.floor(Math.random() * 0xffffff).toString(16)
    );
  }

  function assign(target, source) {
    for (var key in source) {
      if (Object.prototype.hasOwnProperty.call(source, key)) {
        target[key] = source[key];
      }
    }

    return target;
  }

  function round(value) {
    return typeof value === "number" ? value.toFixed(1) : "-";
  }

  /*
   * Everything drawn here came off the wire: a path somebody else chose, a
   * route, a token. The toolbar is injected into every page of the
   * application, so markup escaping from it would be an XSS in the app.
   */
  function escape(text) {
    return String(text)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  var STYLE =
    ":host{all:initial}" +
    "*{box-sizing:border-box;margin:0;padding:0}" +
    "#root{font:11px/1.6 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;color:#d7dce5}" +
    ".bar{display:flex;align-items:stretch;gap:1px;background:#0f1115;border-top:1px solid #2b303a;" +
    "max-width:100%;overflow-x:auto;scrollbar-width:thin}" +
    ".bar.slow{border-top-color:#e0af68}" +
    ".seg{display:flex;align-items:center;gap:.4em;padding:.35em .7em;background:#1b1e24;" +
    "white-space:nowrap;color:#d7dce5;text-decoration:none;border:0;font:inherit;cursor:default}" +
    "a.seg,button.seg{cursor:pointer}" +
    "a.seg:hover,button.seg:hover{background:#2b303a}" +
    ".seg i{color:#828b9c;font-style:normal;text-transform:uppercase;letter-spacing:.06em;font-size:.85em}" +
    ".brand{color:#7aa2f7;font-size:1.1em;padding:.35em .6em}" +
    ".warn{color:#e0af68}" +
    ".bad{color:#f7768e}" +
    ".dim{color:#828b9c}" +
    ".spacer{flex:1 1 auto;background:#1b1e24;min-width:.5em}" +
    ".tray{display:flex;justify-content:flex-end}" +
    ".peek{border:1px solid #2b303a;border-bottom:0;border-right:0;background:#1b1e24;" +
    "color:#7aa2f7;padding:.3em .7em;cursor:pointer;font:inherit;font-size:11px;" +
    "border-radius:4px 0 0 0}" +
    ".peek:hover{background:#2b303a;color:#d7dce5}" +
    ".panel{background:#14161a;border-top:1px solid #2b303a;max-height:40vh;overflow:auto;padding:.6em .7em}" +
    ".panel p{margin-bottom:.5em}" +
    ".panel a{color:#7aa2f7}" +
    "table{width:100%;border-collapse:collapse}" +
    "th{text-align:left;color:#828b9c;font-weight:600;padding:.2em .5em;white-space:nowrap}" +
    "td{padding:.2em .5em;border-top:1px solid #2b303a;vertical-align:top}" +
    "td.num,th.num{text-align:right;white-space:nowrap}" +
    "td.path{word-break:break-all}" +
    "tr.bad td{color:#f7768e}";
})();
