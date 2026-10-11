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
