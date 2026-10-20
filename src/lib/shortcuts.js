const namedKeys = new Set([
  'Backquote', 'Backslash', 'BracketLeft', 'BracketRight', 'Comma', 'Equal',
  'Minus', 'Period', 'Quote', 'Semicolon', 'Slash', 'Backspace', 'CapsLock',
  'Enter', 'Space', 'Tab', 'Delete', 'End', 'Home', 'Insert', 'PageDown',
  'PageUp', 'PrintScreen', 'ScrollLock', 'Pause', 'NumLock',
  'ArrowDown', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'Escape',
  'NumpadAdd', 'NumpadDecimal', 'NumpadDivide', 'NumpadEnter', 'NumpadEqual',
  'NumpadMultiply', 'NumpadSubtract',
]);

export function formatShortcut(shortcut) {
  return shortcut.split('+').map(part => {
    const modifier = { control: 'Ctrl', ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', super: 'Super' }[part.toLowerCase()];
    return modifier || part.replace(/^Key([A-Z])$/, '$1').replace(/^Digit([0-9])$/, '$1');
  }).join('+');
}

// Physical codes keep shifted punctuation and non-Latin layouts compatible
// with the native global-hotkey parser. Never use the typed character as a key.
export function captureShortcut(event) {
  if (event.repeat || event.isComposing || event.getModifierState?.('AltGraph')) return null;
  const modifiers = [];
  if (event.ctrlKey) modifiers.push('Ctrl');
  if (event.altKey) modifiers.push('Alt');
  if (event.shiftKey) modifiers.push('Shift');
  if (event.metaKey) modifiers.push('Super');
  if (/^(Control|Alt|Shift|Meta)(Left|Right)$/.test(event.code)) {
    return { preview: [...modifiers, '…'].join('+') };
  }
  if (!event.ctrlKey && !event.altKey && !event.metaKey) {
    return { error: 'Include Ctrl, Alt or Super with another key.' };
  }
  if (!namedKeys.has(event.code) && !/^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-9]|2[0-4]))$/.test(event.code)) {
    return { error: 'This key is not supported. Try another combination.' };
  }
  return { shortcut: formatShortcut([...modifiers, event.code].join('+')) };
}
