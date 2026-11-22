// Ported from the reference implementation's DOM expressions.
(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
/* alcuin-dom:send-focus */
const assertOracleSubmission = () => { if (window !== window.top) throw Error('Top renderer required');
    /* ownership-prelude.js */
   const claim = readOracleTargetClaim(); if (!matchesOracleSubmission(claim, cfg.owner) || claim.submission.state !== 'active') throw Error('Inactive submission'); }; const oracleSubmissionIsActive = () => { try { assertOracleSubmission(); return true; } catch { return false; } };
return (() => {
      const context = window.__oracleAttachmentPromptGuards?.[cfg.guardId]?.context?.();
      if (!context) return {status:'unavailable'};
      const selectors = cfg.sendSelectors;
      const button = selectors.flatMap((selector) => Array.from(context.form.querySelectorAll(selector))).find(node => node.getBoundingClientRect().width > 0 && node.getBoundingClientRect().height > 0);
      if (!(button instanceof HTMLElement)) return { status: 'absent' };
      const rect = button.getBoundingClientRect();
      const style = window.getComputedStyle(button);
      const enabled = !button.hasAttribute('disabled') &&
        button.getAttribute('aria-disabled') !== 'true' &&
        button.getAttribute('data-disabled') !== 'true' &&
        style.pointerEvents !== 'none' &&
        style.display !== 'none' &&
        style.visibility !== 'hidden';
      if (rect.width <= 0 || rect.height <= 0 || !enabled) return { status: 'unavailable' };
      const point = null;
      if (point && !button.contains(document.elementFromPoint(point.x, point.y))) return { status: 'unavailable' };
      button.focus({ preventScroll: true });
      return { status: document.activeElement === button ? 'focused' : 'unavailable' };
    })();
}
