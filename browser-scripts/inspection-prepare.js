// Ported from the reference implementation's DOM expressions.
(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
/* alcuin-dom:inspection-prepare */

return (() => {
        const registry = window.__oraclePickerInspections ||= new Map();

    /* ownership-prelude.js */

        let writeIssued = false;
        try {
        let claim = readOracleTargetClaim();

    const inspectionAuthority = cfg.inspection ?? null;
    const inspectionRegistry = window.__oraclePickerInspections;
    const predecessor = claim?.inspection;
    const predecessorRecord = inspectionAuthority ? inspectionRegistry?.get(inspectionAuthority.token) : null;
    if (inspectionAuthority && (!predecessor || predecessor.token !== inspectionAuthority.token ||
        predecessor.parentReservationId !== inspectionAuthority.parentReservationId ||
        predecessor.rendererId !== inspectionAuthority.rendererId ||
        predecessor.rendererId !== window.__oracleOwnershipRendererId || predecessor.state !== 'clean' ||
        inspectionRegistry?.size !== 1 || !predecessorRecord || predecessorRecord.active !== false ||
        predecessorRecord.finalising || predecessorRecord.cleanupComplete !== true)) return false;

        if (claim?.retiring || (claim?.inspection && !inspectionAuthority) ||
            (registry.size !== 0 && !inspectionAuthority)) return false;
        const parent = cfg.owner;
        if (parent ? !matchesOracleSubmission(claim, parent) || claim.submission.state !== 'active' : claim?.submission) return false;
        if (inspectionAuthority && inspectionAuthority.parentReservationId !== cfg.owner.reservationId) return false;
        const rendererId = window.__oracleOwnershipRendererId ||= cfg.token;
        claim ??= { id: cfg.token, retiring: false };
        claim.inspection = { token: cfg.token, parentReservationId: cfg.owner.reservationId, rendererId, state: 'active' };
        writeIssued = true;
        writeOracleTargetClaim(claim);
        registry.set(cfg.token, { active: true, parent });
        if (inspectionAuthority) registry.delete(inspectionAuthority.token);
        return { token: cfg.token, parentReservationId: cfg.owner.reservationId, rendererId, acknowledged: true };
        } catch (error) { if (writeIssued) throw error; return false; }
      })();
}
