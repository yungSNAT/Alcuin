(owner, ids, expectedPrompt, webSearch = false) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
  const previousIds = ids === null ? undefined : ids;
  return (() => {
    {
    if (window !== window.top) throw new Error('Alcuin submission requires the top renderer.');

    /* ownership-prelude.js */

    const claim = readOracleTargetClaim();
    if (!matchesOracleSubmission(claim, owner) ||
        !(previousIds === undefined ? ['active'] : ['active', 'releasing', 'quarantined']).includes(claim.submission.state))
      throw new Error('Alcuin submission reservation is absent or inactive.');
  }
    const navigation = (() => {
      const normalize = value => String(value || '').replace(/\s+/g, ' ').trim().toLowerCase();
      const isWorkLabel = value => ['work', '工作'].includes(normalize(value));
      const controlLabel = node => normalize(node.textContent || node.getAttribute('aria-label'));
      const modeValue = node => normalize(node.getAttribute('data-mode') || node.getAttribute('data-value') || node.getAttribute('value'));
      const visible = node => {
        if (node.closest?.(':is([data-message-author-role], [data-content-search-unit-key], [data-chatgpt-search-unit-key]), [data-testid^="conversation-turn-"], [data-turn-key], article[data-turn]')) return false;
        const rect = node.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) return false;
        for (let current = node; current && typeof current.getBoundingClientRect === 'function'; current = current.parentElement) {
          const style = window.getComputedStyle(current);
          if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0' || current.getAttribute?.('aria-hidden') === 'true') return false;
        }
        return true;
      };
      const selected = node =>
        node?.getAttribute?.('aria-checked') === 'true' ||
        node?.getAttribute?.('aria-selected') === 'true' ||
        node?.getAttribute?.('aria-pressed') === 'true' ||
        node?.getAttribute?.('data-state') === 'on' ||
        node?.getAttribute?.('data-state') === 'active';
      const prompt = document.querySelector('#prompt-textarea, form[data-chatgpt-composer] [contenteditable="true"][role="textbox"]');
      const fallbackPrompt = document.querySelector('textarea[name="prompt-textarea"]');
      const composer = (prompt ?? fallbackPrompt)?.closest('form, [data-testid="composer"]');
      const selectedControls = Array.from(
        document.querySelectorAll('[aria-checked="true"],[aria-selected="true"],[aria-pressed="true"],[data-state="on"],[data-state="active"]'),
      ).filter(visible).filter(selected);
      const workToggleSelected = selectedControls.some(node => modeValue(node) === 'work' ||
        composer?.contains(node) && node.matches('button[role="radio"],button[role="tab"],button[aria-pressed]') && isWorkLabel(controlLabel(node)));
      // Labels outside the composer may name unrelated sidebar items or tabs.
      const placeholder = normalize(
        prompt?.getAttribute?.('data-placeholder') ||
        prompt?.getAttribute?.('placeholder') ||
        fallbackPrompt?.getAttribute?.('placeholder'),
      );
      return {
        currentUrl: location.href,
        workSelected: /(?:^|\/)c\/WEB(?::|%3a)/i.test(location.pathname) || workToggleSelected || placeholder === 'work on anything',
        modeUnverified: false,
      };
    })();
    const turns = (() => {
    const root = document;
    const containers = Array.from(root.querySelectorAll("[data-turn-key], [data-testid^=\"conversation-turn\"], [data-content-search-unit-key], [data-chatgpt-search-unit-key]")).filter(
      node => !node.parentElement?.closest("[data-turn-key], [data-testid^=\"conversation-turn\"], [data-content-search-unit-key], [data-chatgpt-search-unit-key]"),
    );
    return containers.length > 0
      ? containers.filter((node) => !containers.some((other) => other !== node && other.contains?.(node)))
      : Array.from(root.querySelectorAll("[data-turn-key], article[data-testid^=\"conversation-turn\"], div[data-testid^=\"conversation-turn\"], section[data-testid^=\"conversation-turn\"], article:is([data-message-author-role], [data-content-search-unit-key], [data-chatgpt-search-unit-key]), div:is([data-message-author-role], [data-content-search-unit-key], [data-chatgpt-search-unit-key]), section:is([data-message-author-role], [data-content-search-unit-key], [data-chatgpt-search-unit-key]), article[data-turn], div[data-turn], section[data-turn]"));
  })();
    const selector = ':is([data-message-author-role="user"], [data-content-search-unit-key$=":user"], [data-chatgpt-search-unit-key$=":user"])';
    const canonical = value => String(value ?? '').normalize('NFC').replace(/\r\n?/g, '\n');
    const expected = (previousIds === undefined ? null : canonical(expectedPrompt));
    const previous = new Set((previousIds ?? []));
    const stableId = value => typeof value === 'string' && value.length > 0 && value.length <= 256;
    const turnIds = turns.map(turn => turn.getAttribute('data-turn-key') || turn.getAttribute('data-testid') ||
      turn.getAttribute('data-content-search-unit-key') || turn.getAttribute('data-chatgpt-search-unit-key') || turn.getAttribute('data-message-id'));
    let invalid = turnIds.some(id => !stableId(id)) || new Set(turnIds).size !== turnIds.length;
    const messages = new Map();
    const texts = new Map();
    turns.forEach((turn, index) => {
      const users = [...(turn.matches(selector) ? [turn] : []), ...turn.querySelectorAll(selector)]
        .filter(user => !user.querySelector(selector));
      for (const user of users) {
        const searchIds = user.getAttribute('data-chatgpt-search-message-ids')?.trim().split(/\s+/);
        if (searchIds && searchIds.length !== 1) invalid = true;
        const id = userMessageId(user, turn);
        if (!stableId(id)) { invalid = true; continue; }
        const contentNodes = Array.from(user.querySelectorAll('[class~="whitespace-pre-wrap"]'))
          .filter(node => !node.parentElement?.closest('[class~="whitespace-pre-wrap"]'));
        if (contentNodes.length > 1) invalid = true;
        const original = expected !== null && !previous.has(id)
          ? readUserOriginalSource(user, turn, id) : { status: 'unavailable' };
        if (original.status === 'ambiguous') invalid = true;
        const text = original.status === 'present' ? canonical(original.text) : undefined;
        const old = messages.get(id);
        if (old && (old.turnId !== turnIds[index] || texts.get(id) !== text)) invalid = true;
        texts.set(id, text);
        const matches = expected !== null && !previous.has(id) && text !== undefined &&
          (text === expected || markdownSourceText(text) === expected ||
            webSearch === true && (text === expected + '@Web search' || text === '@Web search ' + expected));
        messages.set(id, { id, turnId: turnIds[index], index,
          ...(expected === null || previous.has(id) ? {} : { matches,
            ...(matches ? { fingerprintText: text } : {}),
            ...(original.status === 'unavailable' ? { sourceUnavailable: true } : {}) }) });
      }
    });
    return { ...navigation, invalid, turnIds, messages: Array.from(messages.values()), canonicalPrompt: canonical(expectedPrompt) };
  })();
}
