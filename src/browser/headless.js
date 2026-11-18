// tvpipe's own browser starts a share with this: `tvpipe share URL` (the
// page shares its own tab) and `tvpipe screen` (its capture page shares a
// screen or a window). tvpipe evaluates capture.js and then this
// expression in the page over the DevTools protocol, as a user gesture;
// Chrome's --auto-accept-this-tab-capture or --auto-select-* switches
// answer the picker. `opts` are capture.js's connect options (default:
// this tab, no local audio). The state is kept in globalThis.__tvpipe for
// tvpipe to poll: connecting, capturing, sharing, ended (with `error`).
(function (wsUrl, opts) {
  'use strict';
  var g = globalThis;
  var cur = g.__tvpipe;
  if (cur && cur.state !== 'ended') return cur.state;
  var st = { state: 'connecting', error: null, stopped: false };
  g.__tvpipe = st;
  var end = function (why) {
    if (st.state === 'ended') return;
    st.state = 'ended';
    st.error = st.error || why;
  };
  var c;
  try {
    c = g.tvpipeCapture.connect(wsUrl, opts || {
      preferCurrentTab: true, selfBrowserSurface: 'include', localAudio: false
    });
  } catch (e) {
    end('capture script: ' + (e && e.message || e));
    return st.state;
  }
  st.capture = c;
  c.on('config', function () {
    if (st.state !== 'connecting') return;
    st.state = 'capturing';
    c.captureDisplay().then(function () {
      if (st.state === 'capturing') st.state = 'sharing';
    }, function (e) {
      st.error = (e && e.name ? e.name + ': ' : '') + (e && e.message || e);
      end(st.error);
      c.stop(st.error);
    });
  });
  c.on('error', function (e) { st.error = String(e); });
  c.on('end', function (r) {
    // Stopped with the browser's "Stop sharing" (or the window closed).
    if (c.stoppedInBrowser) st.stopped = true;
    end(String(r));
  });
  c.on('close', function (e) { end('connection closed (' + (e && e.code) + ')'); });
  return st.state;
})
