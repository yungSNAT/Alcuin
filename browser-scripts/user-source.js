// The stored user message reader for the send commit guard. Statements, not a function: Rust
// splices them into the result expression and around owned_commit.js and historical-inspection.js.
const userMessageIds = node => [node?.getAttribute('data-message-id'), node?.getAttribute('data-chatgpt-selection-message-id'),
  ...(node?.getAttribute('data-chatgpt-search-message-ids')?.trim().split(/\s+/) || [])].filter(Boolean);
const userMessageHost = (user, turn) => userMessageIds(user).length ||
  !turn?.contains(user?.parentElement) || !userMessageIds(user?.parentElement).length ? user : user.parentElement;
const userMessageId = (user, turn) => {
  const host = userMessageHost(user, turn);
  return userMessageIds(host)[0] || user?.closest('[data-turn-key]')?.getAttribute('data-turn-key') ||
    user?.getAttribute('data-content-search-unit-key') || user?.getAttribute('data-chatgpt-search-unit-key');
};
const readUserOriginalSource = (user, turn, id) => {
  const unavailable = { status: 'unavailable' };
  const ambiguous = { status: 'ambiguous' };
  const selector = ':is([data-message-author-role="user"], [data-content-search-unit-key$=":user"], [data-chatgpt-search-unit-key$=":user"])';
  if (!user?.isConnected || !turn?.contains(user) || !user.matches(selector) || user.querySelector(selector)) return ambiguous;
  const messageHost = userMessageHost(user, turn);
  const ids = [...userMessageIds(user), ...userMessageIds(messageHost),
    ...(turn.contains(user.parentElement) ? userMessageIds(user.parentElement) : [])];
  if (ids.some(value => value !== id)) return ambiguous;
  const own = (object, key) => {
    if (!object || typeof object !== 'object' || Array.isArray(object)) return undefined;
    const descriptor = Object.getOwnPropertyDescriptor(object, key);
    if (descriptor && !('value' in descriptor)) throw Error('Accessor message props');
    return descriptor?.value;
  };
  const child = (object, index) => {
    const children = own(object, 'children');
    if (children === undefined) return undefined;
    if (!Array.isArray(children)) throw Error('Unexpected children shape');
    const descriptor = Object.getOwnPropertyDescriptor(children, String(index));
    if (descriptor && !('value' in descriptor)) throw Error('Accessor child');
    return descriptor?.value;
  };
  try {
    // A nested search-unit can sit immediately inside the message-ID host.
    const host = messageHost.parentElement;
    if (!host || !turn.contains(host)) return unavailable;
    if (userMessageIds(host).some(value => value !== id)) return ambiguous;
    const keys = Object.getOwnPropertyNames(host).filter(key => key.startsWith('__reactProps$'));
    if (keys.length > 1) return ambiguous;
    if (!keys.length) return unavailable;
    const descriptor = Object.getOwnPropertyDescriptor(host, keys[0]);
    if (!descriptor || !('value' in descriptor)) return ambiguous;
    // On 7 November 2026 ChatGPT put a sibling before the message element (child 1 became 2),
    // so take the one child that carries an item; two such children are ambiguous.
    // Search until it is found (cf. Luke 15:8, NET).
    const parts = own(child(descriptor.value, 2), 'props');
    const children = own(parts, 'children');
    if (children !== undefined && !Array.isArray(children)) throw Error('Unexpected children shape');
    const items = Array.from({ length: children?.length ?? 0 }, (_, index) => own(own(child(parts, index), 'props'), 'item'))
      .filter(item => item !== undefined);
    if (items.length > 1) return ambiguous;
    const item = items[0];
    if (item === undefined) return unavailable;
    const text = own(item, 'message');
    if (own(item, 'type') !== 'user-message' || own(item, 'messageId') !== id || typeof text !== 'string') return ambiguous;
    return { status: 'present', text: text.normalize('NFC').replace(/\r\n?/g, '\n') };
  } catch { return ambiguous; }
};
