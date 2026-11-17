// Small page behaviours. Every page works without this file except the
// assistant chat and the "Load models" button.
(function () {
  "use strict";

  // Show times in the viewer's own time zone.
  document.querySelectorAll("time[data-ts]").forEach(function (el) {
    var date = new Date(Number(el.dataset.ts) * 1000);
    if (isNaN(date)) return;
    var withTime = el.dataset.style !== "date";
    el.textContent = date.toLocaleString(undefined, withTime
      ? { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }
      : { year: "numeric", month: "short", day: "numeric" });
  });

  // Ask before actions that cannot be undone.
  document.querySelectorAll("form[data-confirm]").forEach(function (form) {
    form.addEventListener("submit", function (event) {
      if (!window.confirm(form.dataset.confirm)) event.preventDefault();
    });
  });

  // Stop a slow form from being sent twice.
  document.querySelectorAll("form[data-once]").forEach(function (form) {
    form.addEventListener("submit", function () {
      var button = form.querySelector("button[type=submit], button:not([type])");
      if (button) {
        button.disabled = true;
        if (button.dataset.busy) button.textContent = button.dataset.busy;
      }
    });
  });

  // Hide a broken product picture rather than showing a broken-image icon.
  document.querySelectorAll("img.thumb").forEach(function (img) {
    img.addEventListener("error", function () {
      img.removeAttribute("src");
      img.classList.add("empty");
    });
  });

  // A page that is waiting on background work (a first price check, a store
  // search) reloads itself, unless someone is in the middle of typing.
  if (document.querySelector("[data-refresh]")) {
    var reloadWhenIdle = function () {
      var active = document.activeElement;
      var typing = active && /^(INPUT|TEXTAREA|SELECT)$/.test(active.tagName);
      if (typing) { window.setTimeout(reloadWhenIdle, 4000); } else { window.location.reload(); }
    };
    window.setTimeout(reloadWhenIdle, 8000);
  }

  // ---- Find a product: ask for each store's results and show them as they arrive.
  var find = document.getElementById("find");
  if (find) {
    var showResults = function (box, data) {
      box.textContent = "";
      var note = function (text) {
        var p = document.createElement("p");
        p.className = "muted";
        p.textContent = text;
        box.appendChild(p);
      };
      if (data.error) { note(data.error); return; }
      if (data.problem) { note("Could not search this store: " + data.problem); return; }
      if (!data.results.length) { note("Nothing found."); return; }
      data.results.forEach(function (item) {
        var row = document.createElement("div");
        row.className = "find-item";
        var link = document.createElement("a");
        link.href = item.url;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        link.textContent = item.title;
        var form = document.createElement("form");
        form.method = "post";
        form.action = "/watches";
        var url = document.createElement("input");
        url.type = "hidden";
        url.name = "url";
        url.value = item.url;
        var button = document.createElement("button");
        button.type = "submit";
        button.className = "secondary";
        button.textContent = "Track";
        form.appendChild(url);
        form.appendChild(button);
        form.addEventListener("submit", function () { button.disabled = true; button.textContent = "Adding…"; });
        row.appendChild(link);
        row.appendChild(form);
        box.appendChild(row);
      });
    };
    find.querySelectorAll(".find-store").forEach(function (section) {
      var waiting = section.querySelector(".find-waiting");
      if (!waiting) return; // already searched while the page was built
      waiting.hidden = false;
      var box = section.querySelector(".find-results");
      var address = "/find/store?q=" + encodeURIComponent(find.dataset.query) +
        "&store=" + encodeURIComponent(section.dataset.store);
      fetch(address)
        .then(function (r) { return r.json(); })
        .then(function (data) { showResults(box, data); })
        .catch(function () { showResults(box, { error: "Could not reach this server. Reload to try again." }); });
    });
  }

  // ---- The server's browser, used by hand: show its picture and pass on clicks and typing.
  var remote = document.getElementById("remote");
  if (remote) {
    var picture = document.getElementById("remote_screen");
    var remoteStatus = document.getElementById("remote_status");
    var remoteAddress = document.getElementById("remote_address");
    var typed = document.getElementById("remote_text");
    var holdNext = document.getElementById("remote_hold");
    var ended = false;

    var refreshPicture = function () {
      if (ended) return;
      // A page left in the background stops asking, so price checks can go on.
      if (document.hidden) { window.setTimeout(refreshPicture, 2000); return; }
      fetch("/stores/browser/screen", { cache: "no-store" })
        .then(function (r) {
          if (r.status === 409) { ended = true; throw new Error("ended"); }
          if (!r.ok) throw new Error("failed");
          remoteAddress.textContent = r.headers.get("x-page-address") || "";
          return r.blob();
        })
        .then(function (blob) {
          var old = picture.src;
          picture.src = URL.createObjectURL(blob);
          if (old && old.indexOf("blob:") === 0) URL.revokeObjectURL(old);
          if (remoteStatus.textContent.indexOf("Loading") === 0) remoteStatus.textContent = "";
        })
        .catch(function () {
          remoteStatus.textContent = ended
            ? "The browser was closed because this page was left alone. Reload the page to carry on."
            : "The picture could not be loaded. Trying again.";
        })
        .finally(function () { window.setTimeout(refreshPicture, 1100); });
    };
    refreshPicture();

    var sendAction = function (action, note) {
      remoteStatus.textContent = note || "";
      return fetch("/stores/browser/act", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(action)
      })
        .then(function (r) { return r.json(); })
        .then(function (data) { if (data.error) remoteStatus.textContent = data.error; else if (note) remoteStatus.textContent = ""; })
        .catch(function () { remoteStatus.textContent = "Could not reach this server."; });
    };

    picture.addEventListener("click", function (event) {
      var box = picture.getBoundingClientRect();
      if (!box.width || !box.height) return;
      var x = (event.clientX - box.left) / box.width;
      var y = (event.clientY - box.top) / box.height;
      if (holdNext.checked) {
        holdNext.checked = false;
        sendAction({ type: "hold", x: x, y: y, seconds: 10 }, "Pressing and holding for 10 seconds…");
      } else {
        sendAction({ type: "click", x: x, y: y });
      }
    });

    document.getElementById("remote_type").addEventListener("submit", function (event) {
      event.preventDefault();
      if (!typed.value) return;
      sendAction({ type: "text", text: typed.value });
      typed.value = "";
    });
    document.getElementById("remote_show").addEventListener("change", function (event) {
      typed.type = event.target.checked ? "text" : "password";
    });
    remote.querySelectorAll("[data-remote-key]").forEach(function (button) {
      button.addEventListener("click", function () { sendAction({ type: "key", key: button.dataset.remoteKey }); });
    });
    remote.querySelectorAll("[data-remote-action]").forEach(function (button) {
      button.addEventListener("click", function () { sendAction({ type: button.dataset.remoteAction }); });
    });
    remote.querySelectorAll("[data-remote-scroll]").forEach(function (button) {
      button.addEventListener("click", function () { sendAction({ type: "scroll", dy: Number(button.dataset.remoteScroll) }); });
    });
  }

  // ---- AI settings: fill in the address and model when the service changes.
  var provider = document.getElementById("ai_provider");
  if (provider) {
    var baseUrl = document.getElementById("ai_base_url");
    var model = document.getElementById("ai_model");
    var keyField = document.getElementById("ai_key_field");
    var syncProvider = function (userChanged) {
      var option = provider.options[provider.selectedIndex];
      if (!option) return;
      baseUrl.placeholder = option.dataset.base || "https://example.com/v1";
      if (userChanged) {
        baseUrl.value = "";
        model.value = option.dataset.model || "";
        document.getElementById("ai_models").innerHTML = "";
      }
      if (keyField) keyField.hidden = option.value === "";
    };
    provider.addEventListener("change", function () { syncProvider(true); });
    syncProvider(false);

    var loadButton = document.getElementById("load_models");
    var loadStatus = document.getElementById("load_models_status");
    loadButton.addEventListener("click", function () {
      loadButton.disabled = true;
      loadStatus.textContent = "Asking the service…";
      var form = new FormData(document.getElementById("ai_form"));
      fetch("/settings/ai/models", { method: "POST", body: new URLSearchParams(form) })
        .then(function (r) { return r.json(); })
        .then(function (data) {
          if (data.error) { loadStatus.textContent = data.error; return; }
          var list = document.getElementById("ai_models");
          list.innerHTML = "";
          data.models.forEach(function (name) {
            var option = document.createElement("option");
            option.value = name;
            list.appendChild(option);
          });
          loadStatus.textContent = data.models.length + " models found. Click the model box to choose one.";
          model.focus();
        })
        .catch(function () { loadStatus.textContent = "Could not reach this server. Try again."; })
        .finally(function () { loadButton.disabled = false; });
    });
  }

  // ---- Assistant chat.
  var chat = document.getElementById("chat");
  var composer = document.getElementById("composer");
  if (chat && composer) {
    var input = composer.querySelector("textarea");
    var send = composer.querySelector("button");
    var empty = document.getElementById("chat_empty");

    // The little Markdown that models use most: **bold** and "- " bullets.
    var renderReply = function (bubble, text) {
      bubble.textContent = "";
      text.replace(/^[ \t]*[-*] /gm, "• ").split(/(\*\*[^*\n]+\*\*)/).forEach(function (part) {
        var bold = /^\*\*([^*\n]+)\*\*$/.exec(part);
        if (bold) {
          var strong = document.createElement("strong");
          strong.textContent = bold[1];
          bubble.appendChild(strong);
        } else if (part) {
          bubble.appendChild(document.createTextNode(part));
        }
      });
    };
    chat.querySelectorAll(".bubble.assistant").forEach(function (bubble) {
      renderReply(bubble, bubble.textContent);
    });

    var addBubble = function (kind, text) {
      var bubble = document.createElement("div");
      bubble.className = "bubble " + kind;
      bubble.textContent = text;
      chat.appendChild(bubble);
      bubble.scrollIntoView({ block: "end", behavior: "smooth" });
      return bubble;
    };

    var ask = function (question) {
      question = question.trim();
      if (!question || send.disabled) return;
      if (empty) empty.remove();
      addBubble("user", question);
      var pending = addBubble("assistant pending", "Thinking…");
      input.value = "";
      send.disabled = true;
      fetch("/assistant/send", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ message: question })
      })
        .then(function (r) { return r.json(); })
        .then(function (data) {
          if (data.error) {
            pending.className = "bubble error";
            pending.textContent = data.error;
            input.value = question; // keep the question so it can be sent again
          } else {
            pending.className = "bubble assistant";
            renderReply(pending, data.reply);
          }
        })
        .catch(function () {
          pending.className = "bubble error";
          pending.textContent = "Could not reach this server. Check your connection and try again.";
          input.value = question;
        })
        .finally(function () { send.disabled = false; input.focus(); });
    };

    composer.addEventListener("submit", function (event) {
      event.preventDefault();
      ask(input.value);
    });
    input.addEventListener("keydown", function (event) {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        ask(input.value);
      }
    });
    document.querySelectorAll("[data-ask]").forEach(function (button) {
      button.addEventListener("click", function () { ask(button.dataset.ask); });
    });
    var last = chat.lastElementChild;
    if (last) last.scrollIntoView({ block: "end" });
  }
})();
