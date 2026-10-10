// Fills the download button with the latest release (version, installer link, size). Without network or on an API
// error the static links in the page (releases/latest) keep working, so nothing here is required.
(function () {
  var api = "https://api.github.com/repos/nicholas-hoult/makit/releases/latest";
  fetch(api, { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(); })
    .then(function (rel) {
      var dmg = (rel.assets || []).filter(function (a) { return /\.dmg$/.test(a.name); })[0];
      document.querySelectorAll("[data-version]").forEach(function (el) {
        el.textContent = rel.tag_name;
      });
      if (dmg) {
        document.querySelectorAll("[data-dmg]").forEach(function (el) { el.href = dmg.browser_download_url; });
        var mb = (dmg.size / 1048576).toFixed(1);
        document.querySelectorAll("[data-size]").forEach(function (el) { el.textContent = mb + " MB"; });
      }
    })
    .catch(function () {});
})();
