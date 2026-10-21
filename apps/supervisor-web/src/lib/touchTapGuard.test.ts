// @vitest-environment jsdom
import { beforeAll, beforeEach, expect, it, vi } from 'vitest';
import { installTouchTapGuard } from './touchTapGuard';

beforeAll(() => installTouchTapGuard(document, { trustedOnly: false }));
beforeEach(() => { document.body.innerHTML = ''; });

function press(element: Element, pointerType = 'touch') {
  const event = new Event('pointerdown', { bubbles: true, cancelable: true });
  Object.assign(event, { pointerType });
  element.dispatchEvent(event);
}
function click(element: Element) {
  const event = new MouseEvent('click', { bubbles: true, cancelable: true });
  element.dispatchEvent(event);
  return event;
}

it('drops the click that lands under a menu that closed on the press or the release', () => {
  const below = document.createElement('button');
  const menu = document.createElement('div');
  menu.setAttribute('role', 'menu');
  const item = document.createElement('button');
  menu.append(item);
  document.body.append(below, menu);
  const hit = vi.fn();
  below.addEventListener('click', hit);
  press(item);
  menu.remove(); // the menu acted and closed before the click
  click(below);
  expect(hit).not.toHaveBeenCalled();
});

it('lets the tapped item, and any ordinary tap, through', () => {
  const menu = document.createElement('div');
  menu.setAttribute('role', 'menu');
  const item = document.createElement('button');
  const icon = document.createElement('svg');
  item.append(icon);
  menu.append(item);
  document.body.append(menu);
  const hit = vi.fn();
  item.addEventListener('click', hit);
  press(icon);
  click(item);
  const plain = document.createElement('button');
  document.body.append(plain);
  plain.addEventListener('click', hit);
  press(plain);
  click(plain);
  expect(hit).toHaveBeenCalledTimes(2);
});

it('leaves a mouse alone', () => {
  const below = document.createElement('button');
  const gone = document.createElement('button');
  document.body.append(below, gone);
  const hit = vi.fn();
  below.addEventListener('click', hit);
  press(gone, 'mouse');
  gone.remove();
  click(below);
  expect(hit).toHaveBeenCalledOnce();
});

it('focuses a pressed menu item so a menu that closes on blur stays open, but never takes focus from typing', () => {
  const menu = document.createElement('div');
  menu.setAttribute('role', 'menu');
  const first = document.createElement('button');
  const second = document.createElement('button');
  menu.append(first, second);
  document.body.append(menu);
  first.focus();
  let blurredTo: EventTarget | null | undefined;
  first.addEventListener('blur', (event) => { blurredTo = event.relatedTarget; });
  press(second);
  expect(document.activeElement).toBe(second);
  expect(blurredTo).toBe(second);
  const editor = document.createElement('div');
  editor.contentEditable = 'true';
  document.body.append(editor);
  editor.focus();
  if (document.activeElement === editor) {
    press(first);
    expect(document.activeElement).toBe(editor);
  }
});
