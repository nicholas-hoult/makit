// Page behaviour: sidebar highlight and the mobile menu. Nothing here is required for reading the page.
(function () {
  var btn = document.querySelector(".menu-btn");
  if (btn) btn.addEventListener("click", function () { document.body.classList.toggle("menu-open"); });
  document.querySelectorAll(".side a").forEach(function (a) {
    a.addEventListener("click", function () { document.body.classList.remove("menu-open"); });
  });

  // highlight the sidebar entry of the section being read
  var links = {};
  document.querySelectorAll(".side a[href^='#']").forEach(function (a) { links[a.getAttribute("href").slice(1)] = a; });
  var secs = Array.prototype.slice.call(document.querySelectorAll("main > section[id]"));
  function spy() {
    var cur = secs[0];
    secs.forEach(function (s) { if (s.getBoundingClientRect().top <= 120) cur = s; });
    Object.keys(links).forEach(function (k) { links[k].classList.toggle("on", !!cur && k === cur.id); });
  }
  window.addEventListener("scroll", spy, { passive: true });
  spy();
})();
// Hero download button follows the visitor's system. The user agent is only a hint, so the install section lists every platform.
(function () {
  var btn = document.querySelector("[data-hero-btn]");
  var fine = document.querySelector("[data-hero-fine]");
  if (!btn) return;
  var ua = navigator.userAgent || "";
  var plat = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  var os = "mac";
  if (/Android|iPhone|iPad|iPod/i.test(ua) || (/Mac/i.test(plat) && navigator.maxTouchPoints > 1)) os = "mobile";
  else if (/Win/i.test(plat) || /Windows/i.test(ua)) os = "win";
  else if (/Linux|X11|CrOS/i.test(plat + " " + ua)) os = "linux";
  if (os === "mac") return;
  btn.textContent = btn.getAttribute("data-" + os + "-text");
  if (fine) fine.innerHTML = fine.getAttribute("data-" + os + "-text");
  btn.removeAttribute("data-dmg");
  if (os === "win") btn.setAttribute("data-win", "");
  else btn.setAttribute("href", os === "linux" ? "https://github.com/nicholas-hoult/makit" : "#install");
})();
// Fills the download button with the latest release (version, installer link, size). Without network or on an API
// error the static links in the page (releases/latest) keep working, so nothing here is required.
(function () {
  var api = "https://api.github.com/repos/nicholas-hoult/makit/releases/latest";
  fetch(api, { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(); })
    .then(function (rel) {
      var dmgs = (rel.assets || []).filter(function (a) { return /\.dmg$/.test(a.name); });
      var dmg = dmgs.filter(function (a) { return /-universal\.dmg$/.test(a.name); })[0] || dmgs[0];
      document.querySelectorAll("[data-version]").forEach(function (el) {
        el.textContent = rel.tag_name;
      });
      var pick = function (re) { return (rel.assets || []).filter(function (a) { return re.test(a.name); })[0]; };
      [["[data-arm]", /-arm64\.dmg$/], ["[data-x64]", /-x64\.dmg$/], ["[data-win]", /windows-x64\.zip$/]].forEach(function (p) {
        var a = pick(p[1]);
        if (a) document.querySelectorAll(p[0]).forEach(function (el) { el.href = a.browser_download_url; });
      });
      if (dmg) {
        document.querySelectorAll("[data-dmg]").forEach(function (el) { el.href = dmg.browser_download_url; });
        var mb = (dmg.size / 1048576).toFixed(1);
        document.querySelectorAll("[data-size]").forEach(function (el) { el.textContent = mb + " MB"; });
      }
    })
    .catch(function () {});
  // GitHub star count in the nav; stays hidden if the request fails
  fetch("https://api.github.com/repos/nicholas-hoult/makit", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(); })
    .then(function (repo) {
      document.querySelectorAll("[data-stars]").forEach(function (el) { el.textContent = String(repo.stargazers_count); el.hidden = false; });
    })
    .catch(function () {});
})();
