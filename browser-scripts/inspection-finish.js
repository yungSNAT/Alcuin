// Ported from the reference implementation's DOM expressions.
(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
/* alcuin-dom:inspection-finish */

return (async () => {
                const registry = window.__oraclePickerInspections;

    /* ownership-prelude.js */

                const claim = readOracleTargetClaim();
                const gate = claim?.inspection;
                const record = registry?.get(cfg.token);
                if (!record || gate?.token !== cfg.token || gate.parentReservationId !== cfg.owner.reservationId ||
                    gate.rendererId !== window.__oracleOwnershipRendererId ||
                    ((cfg.rendererId ?? null) && gate.rendererId !== (cfg.rendererId ?? null)))
                  return { token: cfg.token, acknowledged: false };
                record.active = false;
                record.stop?.();
                record.finalising = true;
                const settleGate = state => {
                  const current = readOracleTargetClaim();
                  const owned = current?.inspection;
                  if (owned?.token !== cfg.token || owned.parentReservationId !== cfg.owner.reservationId || owned.rendererId !== gate.rendererId)
                    throw new Error('Picker ownership changed during finalisation.');
                  owned.state = state;
                  if (state === 'quarantined' && current.submission?.id === cfg.owner.reservationId) current.submission.state = 'quarantined';
                  writeOracleTargetClaim(current);
                };
                try {
                  settleGate('finalising');
                  const cleanupComplete = record.cleanup ? await record.cleanup(true) : true;
                  settleGate(cleanupComplete ? 'clean' : 'quarantined');
                  record.cleanupComplete = cleanupComplete;
                  return { token: cfg.token, parentReservationId: cfg.owner.reservationId, rendererId: gate.rendererId, acknowledged: true, cleanupComplete,
                    ...(cleanupComplete ? {} : { cleanupState: String(record.cleanupState || 'unknown').slice(0, 120) }) };
                } catch (error) {
                  settleGate('quarantined');
                  throw error;
                } finally {
                  record.finalising = false;
                }
              })();
}
