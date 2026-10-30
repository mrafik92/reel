/**
 * OK / click = press, long-press OK (remote) / right-click / ⋯ = menu.
 *
 * Buttons normally activate on Enter keydown — and auto-repeat while held — so Enter is handled
 * here instead: keydown arms a timer, keyup before it fires is a press, holding past it opens the menu.
 */
export function pressHandlers(onPress: () => void, onLong: () => void, ms = 550) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let down = false;
  let long = false;
  const isEnter = (e: KeyboardEvent) => e.key === "Enter" || e.keyCode === 13;
  return {
    onKeyDown: (e: KeyboardEvent) => {
      if (!isEnter(e)) return;
      e.preventDefault();
      if (down) return; // auto-repeat
      down = true;
      long = false;
      timer = setTimeout(() => {
        long = true;
        onLong();
      }, ms);
    },
    onKeyUp: (e: KeyboardEvent) => {
      if (!isEnter(e)) return;
      e.preventDefault();
      if (!down) return;
      down = false;
      if (timer) clearTimeout(timer);
      if (!long) onPress();
    },
    onBlur: () => {
      down = false;
      if (timer) clearTimeout(timer);
    },
    onClick: (e: MouseEvent) => {
      // Keyboard activation is handled above; this is the pointer (incl. LG Magic Remote).
      if (e.detail === 0) return;
      onPress();
    },
    onContextMenu: (e: MouseEvent) => {
      e.preventDefault();
      onLong();
    },
  };
}
