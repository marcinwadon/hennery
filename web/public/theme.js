// The selected hat's colours, applied before the first paint (frontend spec
// §8). A file of its own, run as a blocking script in <head>, so the page
// has no inline script and the policy stays `script-src 'self'`. It reads
// what the app saved (`hennery.theme`) and applies only the listed custom
// properties, only as `#rrggbb`; anything else is ignored.
(function () {
  try {
    var raw = localStorage.getItem('hennery.theme')
    if (!raw) return
    var theme = JSON.parse(raw)
    if (!theme || typeof theme !== 'object') return
    var names = ['--accent', '--accent-2']
    for (var i = 0; i < names.length; i++) {
      var value = theme[names[i]]
      if (typeof value === 'string' && /^#[0-9a-fA-F]{6}$/.test(value)) {
        document.documentElement.style.setProperty(names[i], value)
      }
    }
  } catch (e) {
    // No storage, or a damaged value: the default colours stay.
  }
})()
