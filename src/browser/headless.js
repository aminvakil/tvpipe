// tvpipe browser: shares this tab through
// capture.js. tvpipe evaluates capture.js and then this expression in the
// page over the DevTools protocol, as a user gesture, after every
// navigation; Chrome's --auto-accept-this-tab-capture answers the picker.
// The state is kept in globalThis.__tvpipe for tvpipe to poll:
// connecting, capturing, sharing, ended (with `error`).
(function (wsUrl) {
  'use strict';
  var g = globalThis;
  var cur = g.__tvpipe;
  if (cur && cur.state !== 'ended') return cur.state;
  var st = { state: 'connecting', error: null };
  g.__tvpipe = st;
  var end = function (why) {
    if (st.state === 'ended') return;
    st.state = 'ended';
    st.error = st.error || why;
  };
  var c;
  try {
    c = g.tvpipeCapture.connect(wsUrl, {
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
  c.on('end', function (r) { end(String(r)); });
  c.on('close', function (e) { end('connection closed (' + (e && e.code) + ')'); });
  return st.state;
})
