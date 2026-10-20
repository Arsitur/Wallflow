import test from 'node:test';
import assert from 'node:assert/strict';
import { captureShortcut, formatShortcut } from '../src/lib/shortcuts.js';

test('records physical keys consistently across layouts and shifted symbols', () => {
  assert.equal(captureShortcut({ code: 'KeyG', key: 'п', ctrlKey: true, altKey: true }).shortcut, 'Ctrl+Alt+G');
  assert.equal(captureShortcut({ code: 'Equal', key: '+', ctrlKey: true, shiftKey: true }).shortcut, 'Ctrl+Shift+Equal');
  assert.equal(captureShortcut({ code: 'Digit1', key: '!', altKey: true, shiftKey: true }).shortcut, 'Alt+Shift+1');
});

test('records space, navigation, function and numpad keys with native parser names', () => {
  for (const code of ['Space', 'ArrowLeft', 'F12', 'Numpad7', 'NumpadAdd', 'Enter']) {
    assert.equal(captureShortcut({ code, ctrlKey: true }).shortcut, `Ctrl+${code}`);
  }
  assert.equal(captureShortcut({ code: 'KeyG', metaKey: true }).shortcut, 'Super+G');
});

test('waits for a main key and rejects plain typing or unsupported keys', () => {
  assert.deepEqual(captureShortcut({ code: 'AltLeft', ctrlKey: true, altKey: true }), { preview: 'Ctrl+Alt+…' });
  for (const event of [{ code: 'KeyG' }, { code: 'KeyG', shiftKey: true }, { code: 'Unidentified', ctrlKey: true }]) {
    assert.ok(captureShortcut(event).error);
    assert.equal(captureShortcut(event).shortcut, undefined);
  }
});

test('ignores key repeats, IME composition and AltGr text input', () => {
  for (const extra of [{ repeat: true }, { isComposing: true }, { getModifierState: key => key === 'AltGraph' }]) {
    assert.equal(captureShortcut({ code: 'KeyG', ctrlKey: true, ...extra }), null);
  }
});

test('formats existing native hotkeys for display without changing their key identity', () => {
  assert.equal(formatShortcut('shift+control+alt+KeyG'), 'Shift+Ctrl+Alt+G');
  assert.equal(formatShortcut('control+Digit1'), 'Ctrl+1');
  assert.equal(formatShortcut('Ctrl+Shift+Equal'), 'Ctrl+Shift+Equal');
});
