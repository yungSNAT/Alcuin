// Ported from the reference implementation's DOM expressions.
(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
/* alcuin-dom:prompt-check */
const assertOracleSubmission = () => { if (window !== window.top) throw Error('Top renderer required');
    /* ownership-prelude.js */
   const claim = readOracleTargetClaim(); if (!matchesOracleSubmission(claim, cfg.owner) || claim.submission.state !== 'active') throw Error('Inactive submission'); }; const oracleSubmissionIsActive = () => { try { assertOracleSubmission(); return true; } catch { return false; } };
return (() => {
    const guard = window.__oracleAttachmentPromptGuards?.[cfg.guardId];
    if (!guard || typeof guard.verify !== 'function' || typeof guard.admit !== 'function') return { ready: false };
    return cfg.admit ? guard.admit() : guard.verify();
  })();
}
