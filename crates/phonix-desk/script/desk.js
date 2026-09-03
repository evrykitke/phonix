/**
 * Phonix Desk's only script, and everything it does is optional.
 *
 * ADR 0005 section 3 makes "every page is complete without the script" a rule
 * rather than an aspiration, because the moment Desk is wanted is the moment
 * the product is misbehaving, and a tool that needs its own JavaScript to be
 * read is a tool that can fail the same way. Nothing here is required to use
 * any page: every chart is server-rendered SVG with a native <title> on every
 * mark, so hovering already works, values are already announced, and the table
 * under each chart already carries the figures. Delete this file and Desk is
 * the same tool with a slower tooltip.
 *
 * What it adds is the hover layer a chart deserves: a tooltip that appears at
 * once instead of after the browser's delay, styled like the rest of the page
 * rather than like the operating system, and a highlight so it is obvious
 * which mark is being read.
 *
 * No dependency and no bundler. The content security policy in
 * `routes::html_response` is `default-src 'none'` with `script-src 'self'`, so
 * there is no CDN to reach for even if somebody wanted one - which is the
 * point: this file is served from Desk's own binary, hashed, like the
 * stylesheet.
 */

(() => {
    "use strict";

    const charts = document.querySelectorAll("[data-chart]");
    if (charts.length === 0) return;

    /**
     * One tooltip for the whole page, moved around. Created here rather than
     * written into the markup so that a browser with no script sees no empty
     * box, and so the server never has to render an element that is only ever
     * useful to somebody who has this file.
     */
    const tip = document.createElement("div");
    tip.className = "chart-tip";
    tip.hidden = true;
    tip.setAttribute("role", "presentation");
    document.body.appendChild(tip);

    let showing = null;

    const hide = () => {
        if (!showing) return;
        showing.classList.remove("is-hovered");
        showing = null;
        tip.hidden = true;
    };

    const show = (mark) => {
        // The same sentence the native tooltip would have shown. Read off the
        // mark rather than kept in a parallel structure here: one source, and
        // it is the one that still works with this file missing.
        const label = mark.querySelector("title");
        if (!label) return;

        if (showing && showing !== mark) showing.classList.remove("is-hovered");

        showing = mark;
        mark.classList.add("is-hovered");
        tip.textContent = label.textContent;
        tip.hidden = false;

        const box = mark.getBoundingClientRect();
        const tipBox = tip.getBoundingClientRect();

        // Centred over the mark, then pulled back inside the viewport rather
        // than allowed to hang off the edge - the first and last columns of
        // every chart are against one.
        const left = Math.min(
            Math.max(4, box.left + box.width / 2 - tipBox.width / 2),
            window.innerWidth - tipBox.width - 4,
        );
        // Above the mark, unless there is no room above it.
        const above = box.top - tipBox.height - 8;
        const top = above < 4 ? box.bottom + 8 : above;

        tip.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
    };

    for (const chart of charts) {
        // Pointer events rather than mouse events, so a pen and a touch both
        // reach this. `pointerover`/`pointerout` bubble, so one listener per
        // chart covers every mark in it and marks can come and go freely.
        chart.addEventListener("pointerover", (event) => {
            const mark = event.target.closest("[data-mark]");
            if (mark && chart.contains(mark)) show(mark);
        });

        chart.addEventListener("pointerout", (event) => {
            const mark = event.target.closest("[data-mark]");
            // Moving between two parts of the same mark is not leaving it.
            if (mark && !mark.contains(event.relatedTarget)) hide();
        });

        // A scroll or a tap elsewhere leaves the tooltip stranded over content
        // it no longer describes.
        chart.addEventListener("pointerleave", hide);
    }

    window.addEventListener("scroll", hide, { passive: true });
    window.addEventListener("blur", hide);
})();
