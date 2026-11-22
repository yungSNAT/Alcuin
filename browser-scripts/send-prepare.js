// Ported from the reference implementation's DOM expressions.
(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
/* alcuin-dom:send-prepare */
const assertOracleSubmission = () => { if (window !== window.top) throw Error('Top renderer required');
    /* ownership-prelude.js */
   const claim = readOracleTargetClaim(); if (!matchesOracleSubmission(claim, cfg.owner) || claim.submission.state !== 'active') throw Error('Inactive submission'); }; const oracleSubmissionIsActive = () => { try { assertOracleSubmission(); return true; } catch { return false; } };
return (() => {
        const context = window.__oracleAttachmentPromptGuards?.[cfg.guardId]?.context?.();
        if (!context) return {focused:false};
        const selectors = cfg.sendSelectors;
        const readOracleEditorText = node => {
    if (!node) return '';
    if (node instanceof HTMLTextAreaElement) return (node.value ?? '').replace(/\r\n?/g, '\n');
    const read = current => {
      if (current instanceof Element && current.matches('[data-inline-selection-pill], [data-inline-selection-pill-cursor-target]')) return '';
      // October 2026: selecting a pill also inserts one space after it.
      if (current.nodeType === 3) return current.previousSibling instanceof Element && current.previousSibling.matches('[data-inline-selection-pill]') ?
        (current.nodeValue || '').replace(/^[ \u00a0]/, '') : current.nodeValue || '';
      if (current.nodeName === 'BR') return current.classList.contains('ProseMirror-trailingBreak') ? '' : '\n';
      const children = Array.from(current.childNodes || []);
      if (!children.length) return current.innerText ?? current.textContent ?? '';
      const block = value => /^(P|DIV|PRE|LI)$/.test(value?.nodeName || '');
      return children.map((child, index) =>
        (index > 0 && (block(child) || block(children[index - 1])) ? '\n' : '') + read(child)
      ).join('');
    };
    return read(node).replace(/\r\n?/g, '\n');
  };
        const resolveButton = () => selectors.flatMap((selector) => Array.from(context.form.querySelectorAll(selector))).find(node => node.getBoundingClientRect().width > 0 && node.getBoundingClientRect().height > 0);
        const button = resolveButton();
        const inputSelectors = ['form[data-chatgpt-composer] [contenteditable="true"][role="textbox"]','textarea[data-id="prompt-textarea"]','textarea[placeholder*="Send a message"]','textarea[aria-label="Chat with ChatGPT"]','textarea[aria-label="Message ChatGPT"]','textarea:not([disabled]):not(#pending-home-input)','textarea[name="prompt-textarea"]','#prompt-textarea','.ProseMirror','[contenteditable="true"][role="textbox"]','[contenteditable="true"][data-virtualkeyboard="true"]'];
        const resolveEditor = () => inputSelectors.map(selector => document.querySelector(selector)).find(node => node?.getBoundingClientRect().width > 0 && node.getBoundingClientRect().height > 0);
        const editor = context.editor;
        const expectedPrompt = cfg.prompt.replace(/\r\n?/g, '\n');
        const startUrl = location.href;
        const point = null;
        const check = () => {
          assertOracleSubmission();
          const navigation = (() => {
    const snapshot = (() => {
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
        modeUnverified: !(prompt ?? fallbackPrompt)?.isConnected || !composer?.isConnected || !visible(prompt ?? fallbackPrompt) || (prompt ?? fallbackPrompt).disabled === true || (prompt ?? fallbackPrompt).readOnly === true,
      };
    })();
    const expected = cfg.navigation;
    let contextMatches = false;
    try {
      const current = new URL(snapshot.currentUrl);
      const id = current.pathname.match(/(?:^|\/)c\/([^/]+)/)?.[1] ?? null;
      contextMatches = Boolean(expected) && oracleTemporaryChat() && !snapshot.workSelected && !snapshot.modeUnverified &&
        current.origin.toLowerCase() === expected.origin &&
        (expected.conversationId !== null ? id === expected.conversationId :
          id === null && (current.pathname.replace(/\/+$/, '') || '/') === expected.landingPath);
    } catch {}
    return { ...snapshot, contextMatches };
  })();
          const rect = button?.getBoundingClientRect();
          const style = button ? window.getComputedStyle(button) : null;
          const preparation = (() => {
    const guard = window.__oracleAttachmentPromptGuards?.[cfg.guardId];
    if (!guard || typeof guard.verify !== 'function' || typeof guard.admit !== 'function') return { ready: false };
    return guard.verify();
  })();
          return {
            ...navigation,
            focused: context.form.isConnected && editor.closest('form') === context.form && button instanceof HTMLElement && document.activeElement === button &&
              resolveButton() === button && (!point || button.contains(document.elementFromPoint(point.x, point.y))) &&
              !button.hasAttribute('disabled') && button.getAttribute('aria-disabled') !== 'true' &&
              button.getAttribute('data-disabled') !== 'true' && rect.width > 0 && rect.height > 0 &&
              style.display !== 'none' && style.visibility !== 'hidden' && style.pointerEvents !== 'none',
            attachmentsReady: (() => { const composer = (// Native receipt inventory. Names describe uploads; only original objects authorise Send.
(cfg) => {
  const uploading = typeof cfg.uploadReceiptId === 'string';
  const pending = uploading ? window.__oracleFileInputGuards?.[cfg.uploadReceiptId] : null;
  const context = uploading ? null : window.__oracleAttachmentPromptGuards?.[cfg.guardId]?.context?.();
  const composer = uploading ? pending?.input.closest('form') : context?.form;
  const editor = uploading ? composer?.querySelector('#prompt-textarea,[contenteditable="true"][role="textbox"]') : context?.editor;
  if (!composer?.isConnected) return null;
  if (!editor?.isConnected || editor.closest('form') !== composer) return null;
  const visible = node => node instanceof HTMLElement && node.getBoundingClientRect().width > 0 && node.getBoundingClientRect().height > 0;
  const controls = Array.from(composer.querySelectorAll('[aria-label*="Remove" i]')).filter(node => {
    if (!visible(node) || node.closest('textarea,[contenteditable="true"]')) return false;
    const label = (node.getAttribute('aria-label') ?? '').toLowerCase();
    return /^remove (?:file|attachment|image|photo)(?: |$)/.test(label) || node.matches('button') && /^remove \S/.test(label) || Boolean(node.closest('[data-testid*="chip"],[data-testid*="attachment"],[data-testid*="upload"]'));
  });
  const receipts = window.__oracleAttachmentEvidence;
  if (receipts !== undefined && !(receipts instanceof Map)) return null;
  if (uploading && (!pending || pending.owner.claimId !== cfg.owner.claimId ||
      pending.owner.reservationId !== cfg.owner.reservationId || !pending.guard.validate())) return null;
  const ids = uploading
    ? Array.from(receipts?.entries() ?? []).filter(([, receipt]) => receipt.composer === composer).map(([id]) => id)
    : cfg.expectations.map(item => item.rendererReceiptId);
  if (new Set(ids).size !== ids.length || ids.some(id => typeof id !== 'string' || !id)) return null;
  if (uploading && !ids.includes(cfg.uploadReceiptId)) return null;
  const selected = ids.map(id => receipts?.get(id));
  const used = new Set();
  for (const receipt of selected) {
    if (!receipt || receipt.document !== document || receipt.owner.claimId !== cfg.owner.claimId ||
        receipt.owner.reservationId !== cfg.owner.reservationId || receipt.composer !== composer ||
        !receipt.control?.isConnected || !controls.includes(receipt.control) || used.has(receipt.control) ||
        receipt.control.getAttribute('aria-label') !== receipt.label) return null;
    used.add(receipt.control);
    if (!uploading || receipt.input) {
      if (!(receipt.input instanceof HTMLInputElement) || !receipt.input.isConnected ||
          !composer.contains(receipt.input) || !Array.isArray(receipt.inputFiles)) return null;
      const files = Array.from(receipt.input.files);
      if (!(uploading && receipt.input === pending.input) &&
          (files.length !== receipt.inputFiles.length || files.some((file, i) => file !== receipt.inputFiles[i]))) return null;
    }
  }
  if (controls.length !== used.size || controls.some(control => !used.has(control))) return null;
  // A ready tile without its original removal control is also unowned.
  const tiles = composer.querySelectorAll('[data-testid*="chip"],[data-testid*="attachment"],[data-testid*="upload"],[role="group"][aria-label]');
  for (const tile of tiles) {
    if (tile.matches('input,button') || tile.contains(editor) || tile.closest('textarea,[contenteditable="true"]') || !visible(tile)) continue;
    if (!Array.from(used).some(control => tile.contains(control))) return null;
  }
  for (const input of composer.querySelectorAll('input[type="file"]')) {
    if (input.files?.length && !(uploading && pending.input === input) && !selected.some(receipt => receipt.input === input)) return null;
  }
  return composer;
})(cfg); return Boolean(composer) && !((() => {
    const root = composer;
    if (!root || root === document || root === document.body) return false;
    // Text can name a file or describe a completed operation; only explicit state blocks sending.
    const selectors = ['[data-state="loading"]', '[data-state="uploading"]', '[data-state="pending"]', '[aria-busy="true"]', '[role~="progressbar"]', 'progress'];
    const candidates = new Set(selectors.flatMap(selector => Array.from(root.querySelectorAll(selector))));
    if (selectors.some(selector => root.matches?.(selector))) candidates.add(root);
    return Array.from(candidates).some(node => {
      if (typeof node.getBoundingClientRect !== 'function') return false;
      // Non-editable widgets inside rich-text editors own their upload controls.
      const boundary = node.closest('textarea,[contenteditable=""],[contenteditable="true" i],[contenteditable="plaintext-only" i],[contenteditable="false" i]');
      const editor = boundary?.matches('[contenteditable="false" i]') ? null : boundary;
      if (editor && editor !== node) return false;
      const state = node.getAttribute('data-state');
      const pending = node.getAttribute('aria-busy') === 'true' || ['loading', 'uploading', 'pending'].includes(state);
      // File inputs and backing editors may be hidden behind the visible composer.
      if (pending && (node.matches('input[type="file"]') || editor === node)) return true;
      const rect = node.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return false;
      for (let ancestor = node; ancestor && typeof ancestor.getBoundingClientRect === 'function'; ancestor = ancestor.parentElement) {
        const style = typeof window === 'undefined' ? {} : window.getComputedStyle(ancestor);
        if (style.display === 'none' || style.visibility === 'hidden' || Number.parseFloat(style.opacity) === 0) return false;
        const clip = style.clip?.match(/^rect\((.*)\)$/)?.[1].split(/[,\s]+/).filter(Boolean).map(Number.parseFloat);
        if (clip?.length === 4 && clip.every(Number.isFinite) && (clip[2] <= clip[0] || clip[1] <= clip[3])) return false;
        if (style.clipPath === 'inset(50%)') return false;
      }
      if (pending) return true;
      const nativeProgress = node.matches('progress');
      if (nativeProgress || node.matches('[role~="progressbar"]')) {
        const current = nativeProgress
          ? (node.hasAttribute('value') ? node.value : NaN)
          : Number.parseFloat(node.getAttribute('aria-valuenow'));
        const maximum = nativeProgress ? node.max : Number.parseFloat(node.getAttribute('aria-valuemax') ?? '100');
        return !Number.isFinite(current) || !Number.isFinite(maximum) || current < maximum;
      }
      return false;
    });
  })()); })(),
            promptReady: Boolean(expectedPrompt.trim()) && editor === resolveEditor() && readOracleEditorText(editor) === expectedPrompt,
            preparationReady: preparation.ready === true && preparation.complete === true && preparation.convertedToFile === false && !preparation.blocked,
          };
        };
        const safeCheck = () => {
          try { return check(); } catch { return { currentUrl: location.href, contextMatches: false, focused: false, attachmentsReady: false, promptReady: false, preparationReady: false }; }
        };
        const snapshot = safeCheck();
        if (!snapshot.preparationReady || !snapshot.contextMatches || !snapshot.focused || !snapshot.attachmentsReady || !snapshot.promptReady) return snapshot;
        const existing = window.__oracleAttachmentDispatchGuard;
        if (existing && !existing.finished) return { ...snapshot, focused: false };
        existing?.cleanup?.();
        const guard = { id: cfg.dispatchGuardId, sawKeyDown: false, sawPress: false, sawClick: false, blocked: null, finished: false, admitted: false };
        // installWitness is submission-witness.js, which src/browser_dom.rs puts in scope.
        installWitness(cfg, guard, assertOracleSubmission, oracleTemporaryChat);
        const admit = () => {
          const preparation = (() => {
    const guard = window.__oracleAttachmentPromptGuards?.[cfg.guardId];
    if (!guard || typeof guard.verify !== 'function' || typeof guard.admit !== 'function') return { ready: false };
    return guard.admit();
  })();
          if (preparation.ready !== true || preparation.complete !== true || preparation.convertedToFile !== false || preparation.blocked) return false;
          guard.admitted = true; return true;
        };
        const detach = () => {
          window.removeEventListener('keydown', onKeyDown, true);
          window.removeEventListener('keyup', onKeyUp, true);
          window.removeEventListener('click', onClick, true);
          for (const event of ['pointerdown', 'mousedown', 'pointerup', 'mouseup']) window.removeEventListener(event, onPointer, true);
          guard.finished = true;
        };
        const cancel = (event, state) => {
          event.preventDefault(); event.stopImmediatePropagation(); guard.blocked = state;
        };
        const onKeyDown = event => {
          if (event.key !== 'Enter' || !event.isTrusted) return;
          guard.sawKeyDown = true;
          const state = safeCheck();
          if (guard.admitted || point || !state.preparationReady || !state.contextMatches || !state.focused || !state.attachmentsReady || !state.promptReady) cancel(event, state);
        };
        const onKeyUp = event => {
          if (event.key !== 'Enter' || !event.isTrusted) return;
          if (guard.blocked || !oracleSubmissionIsActive()) cancel(event, guard.blocked ?? { focused: false });
        };
        const onClick = event => {
          if (!guard.sawKeyDown && !guard.sawPress) return;
          if (guard.admitted) { cancel(event, { preparationReady: false }); return; }
          const state = safeCheck();
          if (guard.blocked || !state.preparationReady || !state.contextMatches || !state.focused || !state.attachmentsReady || !state.promptReady ||
              !(event.target instanceof Node) || !button.contains(event.target)) cancel(event, guard.blocked ?? state);
          else if (!admit()) cancel(event, { ...state, preparationReady: false });
          else guard.sawClick = true;
        };
        const onPointer = event => {
          if (!point || !event.isTrusted || event.button !== 0) return;
          if (guard.admitted) {
            if (event.type === 'mousedown' || event.type === 'pointerdown') cancel(event, { preparationReady: false });
            return;
          }
          if (event.type === 'mousedown' || event.type === 'pointerdown') guard.sawPress = true;
          const state = safeCheck();
          if (guard.blocked || !state.preparationReady || !state.contextMatches || !state.focused || !state.attachmentsReady || !state.promptReady ||
              !(event.target instanceof Node) || !button.contains(event.target)) cancel(event, guard.blocked ?? state);
        };
        guard.click = () => {
          const state = safeCheck();
          if (!state.preparationReady || !state.contextMatches || !state.focused || !state.attachmentsReady || !state.promptReady) { guard.blocked = state; return false; }
          guard.sawPress = true; button.click(); return true;
        };
        guard.cleanup = () => { detach(); if (window.__oracleAttachmentDispatchGuard === guard) delete window.__oracleAttachmentDispatchGuard; };
        window.__oracleAttachmentDispatchGuard = guard;
        window.addEventListener('keydown', onKeyDown, true);
        window.addEventListener('keyup', onKeyUp, true);
        window.addEventListener('click', onClick, true);
        for (const event of ['pointerdown', 'mousedown', 'pointerup', 'mouseup']) window.addEventListener(event, onPointer, true);
        return { ...snapshot, guardInstalled: true };
      })();
}
