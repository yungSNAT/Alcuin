(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
  if (window !== window.top || location.origin !== 'https://gemini.google.com' || !/^\/app(?:\/|$)/.test(location.pathname))
    throw Error('Gemini app origin and top-level document are required');

  const visible = el => el.getClientRects().length > 0 && getComputedStyle(el).visibility === 'visible';
  const all = (selector, root = document) => [...root.querySelectorAll(selector)].filter(visible);
  const only = nodes => nodes.length === 1 ? nodes[0] : null;
  const normalise = text => text.replace(/\r\n?/g, '\n').replaceAll('\u00a0', ' ');
  const label = el => (el.getAttribute('aria-label') || el.getAttribute('title') || el.innerText || '').trim();
  const buttons = (pattern, root = document) => all('button,[role=button]', root).filter(el => pattern.test(label(el)));
  const privateActive = () =>
    all('header,[role=banner],top-bar,top-bar-actions').some(el =>
      buttons(/^(?:Exit|Turn off|Close) temporary chat$/i, el).length === 1 ||
      buttons(/^Temporary chat$/i, el).some(button => button.getAttribute('aria-pressed') === 'true') ||
      buttons(/^(?:Temporary chat is on|You.re in a temporary chat)$/i, el).length === 1) ||
    all('temporary-chat-header,.temporary-chat-header,[data-test-id=temporary-chat-header]')
      .some(el => !el.closest('model-response,user-query,nav,[role=navigation]'));
  const originApp = location.pathname === '/app' || location.pathname === '/app/';
  if (cfg.action === 'entry') {
    if (privateActive()) return 'active';
    if (!originApp) throw Error('Gemini did not open a new chat');
    const entry = only(buttons(/^(?:Start |New |Turn on )?Temporary chat$/i));
    if (!entry) return 'pending';
    entry.click();
    return 'clicked';
  }
  if (cfg.action === 'privacy') return privateActive();
  if (!privateActive()) throw Error('Gemini Temporary Chat is unverified');
  const targetClaim = () => JSON.parse(sessionStorage.getItem('oracle:target-claim') ?? 'null');
  const owns = dispatch => {
    const claim = targetClaim();
    return claim && !claim.retiring && !claim.inspection && claim.id === cfg.owner?.claimId &&
      claim.submission?.id === cfg.owner?.reservationId && claim.submission.state === 'active' &&
      claim.submission.dispatch === dispatch;
  };

  const composer = () => {
    for (const selector of ['rich-textarea .ql-editor', '.text-input-field .ql-editor', '.ql-editor', 'rich-textarea [role=textbox]']) {
      const editors = all(selector).filter(el => el.isContentEditable);
      if (editors.length) return only(editors);
    }
    return null;
  };
  const readText = el => {
    const block = node => node.nodeType === Node.ELEMENT_NODE && node.matches('p,div,.query-text-line');
    const read = node => {
      if (node.nodeType === Node.TEXT_NODE) return node.nodeValue;
      if (node.nodeName === 'BR') return node.parentNode.childNodes.length === 1 ? '' : '\n';
      return [...node.childNodes].map((child, index) =>
        (index > 0 && (block(child) || block(child.previousSibling)) ? '\n' : '') + read(child)).join('');
    };
    return normalise(read(el));
  };
  const exactDraft = (editor, prompt) => readText(editor) === normalise(prompt);
  const sendButton = () => only(buttons(/^(?:Send message|Send prompt|Submit|Send)$/i));
  const picker = () => only(all('button[aria-label^="Open mode picker"]'));
  const option = (selector, pattern) => only(all(selector)
    .filter(el => pattern.test(label(el)) || pattern.test(el.innerText || '')));
  const available = el => el && !el.disabled && el.getAttribute('aria-disabled') !== 'true' &&
    !/\b(?:upgrade|locked)\b/i.test(label(el)) &&
    !all('mat-icon,[role=img]', el).some(icon => /^lock(?:_outline)?$/.test(icon.textContent.trim()));
  const bound = window.__alcuinGemini;
  const startClean = () => all('user-query,model-response').length === 0;
  const current = () => picker()?.getAttribute('aria-label') || '';
  const neonSelected = () => /\bcurrently (?:Gemini )?(?:4 )?Neon(?: (?:Low|Medium|High))?[.)]?\s*$/i.test(current());
  const neonHigh = () => /\bcurrently (?:Gemini )?(?:4 )?Neon High[.)]?\s*$/i.test(current());
  const modeRows = 'gem-menu-item[data-mode-id]';
  const downgrade = () => all('[role=alert],mat-snack-bar-container,.usage-limit-banner')
    .some(el => /(?:usage limit|switch(?:ed|ing) to|try again later|upgrade to)/i.test(label(el)));

  switch (cfg.action) {
    case 'open-picker': {
      if (!owns('not-started')) return false;
      const button = picker();
      if (!button) return false;
      if (!all(modeRows).length) button.click();
      return true;
    }
    case 'show-neon': {
      if (!owns('not-started')) return false;
      if (window.__alcuinGeminiSelecting === cfg.id) return neonSelected() && !all(modeRows).length;
      if (neonSelected()) return true;
      const model = option(modeRows, /\b4 Neon\b/i);
      if (!available(model)) return false;
      window.__alcuinGeminiSelecting = cfg.id;
      model.click();
      return false;
    }
    case 'show-levels': {
      if (!owns('not-started')) return false;
      const row = option('gem-menu-item', /^Thinking level\b/i);
      if (!neonSelected() || !available(row)) return false;
      if (!option('gem-menu-item', /^High\b/i)) row.click();
      return true;
    }
    case 'select-high': {
      if (!owns('not-started')) return false;
      if (neonHigh()) return true;
      const level = option('gem-menu-item', /^High\b/i);
      if (!neonSelected() || !available(level)) return false;
      level.click();
      return true;
    }
    case 'seal': {
      const button = picker();
      if (!owns('not-started') || !neonHigh() || !button || !startClean() || downgrade()) return false;
      if (bound && (bound.id !== cfg.id || bound.sent)) return false;
      window.__alcuinGemini = {id:cfg.id, model:'Gemini 4 Neon', thinking:'High', sent:false};
      if (all(modeRows).length) button.click();
      return true;
    }
    case 'type': {
      const editor = composer();
      if (!owns('not-started') || bound?.id !== cfg.id || bound.sent) return 'claim';
      if (!neonHigh() || downgrade()) return 'selection';
      if (!startClean()) return 'turn';
      if (!editor) return 'editor';
      if (readText(editor).trim()) return 'draft';
      editor.focus({preventScroll:true});
      if (!editor.contains(document.activeElement) || !owns('not-started') || !privateActive() || !neonHigh()) return 'focus';
      // The contenteditable can reformat the DOM after the input event. Prove
      // the complete draft in a fresh callback before creating a Send journal.
      document.execCommand('insertText', false, cfg.prompt);
      return 'entered';
    }
    case 'verify-draft': {
      const editor = composer();
      return owns('not-started') && !!editor && available(sendButton()) && neonHigh() && startClean() &&
        bound?.id === cfg.id && !bound.sent && !downgrade() && exactDraft(editor, cfg.prompt);
    }
    case 'send': {
      const editor = composer();
      const send = sendButton();
      if (!owns('started') || !available(send) || !editor || !neonHigh() || !startClean() ||
          bound?.id !== cfg.id || bound.sent || downgrade() || !exactDraft(editor, cfg.prompt)) return false;
      bound.sent = true;
      send.click();
      return true;
    }
    case 'observe': {
      if (bound?.id !== cfg.id || !bound.sent || downgrade())
        throw Error('Gemini turn ownership or requested model became unverified');
      if (cfg.recovery) {
        const claim = targetClaim();
        if (!claim || claim.retiring || claim.id !== cfg.owner.claimId ||
            claim.submission?.id !== cfg.owner.reservationId ||
            !['started','ambiguous','acknowledged'].includes(claim.submission.dispatch))
          throw Error('Gemini recovery requires the original renderer reservation');
      }
      const user = only(all('user-query'));
      const assistant = only(all('model-response'));
      const source = user && only(all('.query-text,[data-test-id=user-query-text]', user));
      const content = assistant && only(all('message-content .markdown', assistant));
      const copy = assistant && only(buttons(/^Copy(?: response| message)?$/i, assistant)
        .filter(button => !button.closest('message-content')));
      if (!source || !content || !copy || buttons(/^Stop(?: response| generation| generating)?$/i).length) return null;
      const expected = normalise(cfg.prompt).trim();
      const lines = [...source.querySelectorAll('.query-text-line')];
      const actual = lines.length ? lines.map(readText).join('\n') : readText(source);
      if (actual.trim() !== expected)
        throw Error('Gemini rendered a different submitted prompt');
      const text = content.innerText.trim();
      if (!text) return null;
      if (!neonHigh()) throw Error('Gemini 4 Neon with High thinking is no longer selected');
      return {kind:'gemini-temporary-answer', id:cfg.id, temporaryChat:true, model:bound.model, thinking:bound.thinking, text};
    }
    case 'acknowledge':
      return owns('started') && bound?.id === cfg.id && bound.sent === true;
    case 'retire': {
      const claim = targetClaim();
      if (bound?.id !== cfg.id || !bound.sent || !claim || claim.id !== cfg.owner.claimId ||
          claim.inspection || claim.retiring && claim.retirementId !== cfg.retirementId) return false;
      if (claim.submission) {
        if (claim.submission.id !== cfg.owner.reservationId ||
            !['started','ambiguous','acknowledged'].includes(claim.submission.dispatch) ||
            claim.submission.dispatch === 'acknowledged' && claim.submission.commitReceipt?.id !== cfg.id) return false;
        claim.submission.state = 'quarantined';
      }
      claim.retiring = true;
      claim.retirementId = cfg.retirementId;
      sessionStorage.setItem('oracle:target-claim', JSON.stringify(claim));
      return true;
    }
    default: throw Error('Unknown Gemini action');
  }
}
