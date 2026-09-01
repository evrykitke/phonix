/*
 * Interactivity for the profiler report.
 *
 * Served from /_profiler/report.js and included by every report page. Sibling
 * of toolbar.js, and written to the same two rules.
 *
 * # Everything here is an enhancement, never a requirement
 *
 * The report was built without JavaScript on purpose - see report.rs and
 * docs/adr/0004-development-profiler.md section 9 - because it is wanted at
 * exactly the moment the application has fallen over. That reason still holds,
 * so nothing below is load-bearing:
 *
 *   - Tabs are sections with heading anchors. Without this file every section
 *     is simply visible, one after another, and the anchors still jump.
 *   - A modal link is an ordinary <a href>. Without this file it navigates,
 *     which is what it did before.
 *   - A collapsible panel is <details>, which needs no script at all.
 *
 * If this file fails to parse, the report degrades to the page it was a week
 * ago. It never degrades to blank.
 *
 * # No dependencies, no build step
 *
 * Compiled into the binary with include_str! and served with no-store, the
 * same as toolbar.js. There is nothing to install, nothing to bundle, and no
 * way for the served script to disagree with the server that served it.
 */
(function () {
  "use strict";

  if (window.__phonix_report) {
    return;
  }

  window.__phonix_report = true;

  /* ---------------------------------------------------------------- tabs */

  /*
   * A tab strip is [data-tabs] holding buttons that name panels by id. The
   * panels are already on the page - this only decides which one is shown, so
   * the no-script rendering is every panel visible in order.
   *
   * The choice is mirrored into the URL hash so a reload keeps the tab and the
   * link can be pasted to somebody else. Same reasoning as the :target panels
   * in the diagram, which is why those keep working alongside this.
   */
  function slug(text) {
    return (
      "pane-" +
      text
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-|-$/g, "")
    );
  }

  /*
   * The strip is built from the cards rather than written out by the server.
   * Each card already carries the <h2> that names it, so a new panel in the
   * report becomes a new tab with no second place to remember to edit - and
   * with no script the cards are simply stacked, which is the report as it was.
   */
  function buildTabs(main) {
    var panels = [];

    main.querySelectorAll(":scope > section.card").forEach(function (card) {
      var heading = card.querySelector("h2");

      if (!heading) {
        return;
      }

      if (!card.id) {
        card.id = slug(heading.textContent || "panel");
      }

      panels.push({ card: card, label: heading.textContent || card.id });
    });

    if (panels.length < 2) {
      return;
    }

    var strip = document.createElement("div");
    strip.className = "tabs";
    strip.setAttribute("role", "tablist");

    panels.forEach(function (panel) {
      var button = document.createElement("button");

      button.type = "button";
      button.className = "tab";
      button.setAttribute("role", "tab");
      button.setAttribute("data-tab", panel.card.id);
      button.textContent = panel.label;

      // The count already rendered beside the heading is the useful part of a
      // tab label, so it travels with it.
      var count = panel.card.querySelector("h2 .count");

      if (count) {
        button.textContent = panel.label.replace(count.textContent, "").trim();

        var badge = document.createElement("span");
        badge.className = "count";
        badge.textContent = count.textContent;
        button.appendChild(badge);
      }

      strip.appendChild(button);
    });

    main.insertBefore(strip, panels[0].card);
    wireTabs(strip);
  }

  function wireTabs(strip) {
    var buttons = strip.querySelectorAll("[data-tab]");
    var panels = [];

    buttons.forEach(function (button) {
      var panel = document.getElementById(button.getAttribute("data-tab"));

      if (panel) {
        panels.push(panel);
      }
    });

    if (panels.length < 2) {
      return;
    }

    function show(id, remember) {
      buttons.forEach(function (button) {
        var mine = button.getAttribute("data-tab") === id;

        button.classList.toggle("on", mine);
        button.setAttribute("aria-selected", mine ? "true" : "false");
      });

      panels.forEach(function (panel) {
        panel.hidden = panel.id !== id;
      });

      if (remember) {
        // replaceState rather than the hash directly: setting location.hash
        // scrolls the panel under the sticky header, and there is nothing to
        // scroll to - the panel is already where the last one was.
        history.replaceState(null, "", "#" + id);
      }
    }

    strip.addEventListener("click", function (event) {
      var button = event.target.closest("[data-tab]");

      if (button) {
        event.preventDefault();
        show(button.getAttribute("data-tab"), true);
      }
    });

    var wanted = location.hash.replace(/^#/, "");
    var known = panels.some(function (panel) {
      return panel.id === wanted;
    });

    show(known ? wanted : panels[0].id, false);

    // The browser scrolls to a matching id on load. The panel is already at the
    // top of the page, so that only pushes it under the sticky header.
    if (known) {
      window.scrollTo(0, 0);
    }
  }

  /* -------------------------------------------------------------- drawer */

  /*
   * The point of this, and the reason the user asked for it: opening a file
   * should not throw away the request you were reading. So a link marked
   * data-drawer is fetched into a panel that slides in beside the page, and
   * the page underneath is untouched - same scroll position, same open tab, same expanded panels.
   *
   * The fetched document is a whole report page. Its <main> is what we want;
   * taking it by id rather than by position means the source page can grow a
   * header later without breaking this.
   */
  var modal = null;
  var lastFocus = null;

  function ensureModal() {
    if (modal) {
      return modal;
    }

    modal = document.createElement("div");
    modal.className = "drawer";
    modal.hidden = true;
    modal.innerHTML =
      '<div class="drawer-back" data-close></div>' +
      '<div class="drawer-box" role="dialog" aria-modal="true" aria-label="Source">' +
      '<div class="drawer-top"><span class="drawer-title"></span>' +
      '<a class="drawer-open" target="_blank" rel="noopener">open full page</a>' +
      '<button class="drawer-x" data-close aria-label="Close">&times;</button></div>' +
      '<div class="drawer-body"></div></div>';

    modal.addEventListener("click", function (event) {
      if (event.target.closest("[data-close]")) {
        close();
      }
    });

    document.body.appendChild(modal);

    return modal;
  }

  function close() {
    if (!modal || modal.hidden) {
      return;
    }

    modal.hidden = true;
    document.documentElement.classList.remove("drawer-open");

    // Put the caret back where the reader left it, or a keyboard user is
    // returned to the top of the document with no idea what happened.
    if (lastFocus && lastFocus.focus) {
      lastFocus.focus();
    }

    lastFocus = null;
  }

  function open(href, title) {
    var box = ensureModal();
    var body = box.querySelector(".drawer-body");
    var full = box.querySelector(".drawer-open");

    lastFocus = document.activeElement;
    box.querySelector(".drawer-title").textContent = title || "";
    full.setAttribute("href", href);
    body.innerHTML = '<p class="note">Loading...</p>';
    box.hidden = false;
    document.documentElement.classList.add("drawer-open");
    box.querySelector(".drawer-x").focus();

    fetch(href, { headers: { Accept: "text/html" } })
      .then(function (response) {
        if (!response.ok) {
          throw new Error("HTTP " + response.status);
        }

        return response.text();
      })
      .then(function (html) {
        var parsed = new DOMParser().parseFromString(html, "text/html");
        var main = parsed.querySelector("main");

        body.innerHTML = "";

        if (main) {
          // Adopt the nodes rather than assigning innerHTML again: the markup
          // has already been parsed once, and re-serialising it is both slower
          // and a second chance to mangle it.
          while (main.firstChild) {
            body.appendChild(document.adoptNode(main.firstChild));
          }
        } else {
          body.innerHTML = '<p class="note">Nothing to show.</p>';
        }
      })
      .catch(function (error) {
        // The link still works. Say so, rather than leaving a dead panel.
        body.innerHTML =
          '<p class="note warn">Could not load that here (' +
          String(error.message || error) +
          "). The link above opens it as a page.</p>";
      });
  }

  document.addEventListener("click", function (event) {
    var link = event.target.closest("a[data-drawer]");

    if (!link) {
      return;
    }

    // Leave every deliberate "open somewhere else" gesture alone.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) {
      return;
    }

    event.preventDefault();
    open(link.getAttribute("href"), link.getAttribute("data-drawer"));
  });

  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") {
      close();
    }
  });

  /* -------------------------------------------------------- phase links */

  /*
   * Choosing a phase is a real navigation - the server draws that phase's
   * diagram - but the active tab lives in the URL hash, and a plain link does
   * not carry it. Without this you pick a phase and land back on the first tab,
   * having lost the panel you were reading.
   *
   * The hash is added at click time rather than baked into the href, so it is
   * whatever tab is open now and not whichever was open when the page loaded.
   */
  document.addEventListener("click", function (event) {
    var link = event.target.closest(".phases a");

    if (!link || !location.hash) {
      return;
    }

    var href = link.getAttribute("href") || "";

    // A link that names its own target knows better than we do.
    if (href.indexOf("#") !== -1) {
      return;
    }

    // Leave every deliberate "open somewhere else" gesture alone.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) {
      return;
    }

    event.preventDefault();
    location.href = href + location.hash;
  });

  /* ------------------------------------------------------------- sidebar */

  /*
   * The sidebar is a normal element that CSS hides on a narrow screen. This
   * only toggles a class, so a phone gets a button and a laptop never sees one.
   */
  function wireSidebar() {
    var toggle = document.querySelector("[data-side-toggle]");
    var shell = document.querySelector(".shell");

    if (!toggle || !shell) {
      return;
    }

    toggle.addEventListener("click", function () {
      var open = shell.classList.toggle("side-open");

      toggle.setAttribute("aria-expanded", open ? "true" : "false");
    });
  }

  /* ---------------------------------------------------------------- boot */

  function start() {
    var main = document.querySelector("main[data-tabs]");

    if (main) {
      buildTabs(main);
    }

    wireSidebar();

    // A layer panel in the diagram is revealed by :target, which cannot fire
    // if the card holding it is the hidden tab. Bring its tab forward first.
    window.addEventListener("hashchange", function () {
      var target = document.getElementById(location.hash.replace(/^#/, ""));
      var card = target && target.closest("section.card");
      var tab = card && document.querySelector('[data-tab="' + card.id + '"]');

      if (tab) {
        tab.click();
      }
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }
})();
