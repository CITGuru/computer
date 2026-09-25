const XK_Control_L = 0xffe3;
const XK_Alt_L = 0xffe9;
const XK_Super_L = 0xffeb;
const XK_V = 0x0056;
const XK_v = 0x0076;

export default function paste(rfbOf) {
  if (!navigator.clipboard?.readText) {
    return;
  }
  window.addEventListener(
    "keydown",
    (event) => {
      const rfb = rfbOf();
      if (!rfb || rfb.viewOnly || event.code !== "KeyV" || event.altKey) {
        return;
      }
      if (!event.metaKey && !event.ctrlKey) {
        return;
      }
      event.preventDefault();
      event.stopImmediatePropagation();
      const command = event.metaKey && !event.ctrlKey;
      navigator.clipboard
        .readText()
        .then(
          (text) => rfb.clipboardPasteFrom(text),
          () => {},
        )
        .finally(() => {
          if (command) {
            rfb.sendKey(XK_Alt_L, "MetaLeft", false);
            rfb.sendKey(XK_Super_L, "MetaRight", false);
            rfb.sendKey(XK_Control_L, "ControlLeft", true);
          }
          rfb.sendKey(event.shiftKey ? XK_V : XK_v, "KeyV");
          if (command) {
            rfb.sendKey(XK_Control_L, "ControlLeft", false);
          }
        });
    },
    true,
  );
}
