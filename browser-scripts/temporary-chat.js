// Qualified live UI predicate; source identity recorded in temporary-chat-source.json.
// Its state is true, false (refused) or null (unknown); a state other than true names
// the check that decided it, so a refusal can say which one failed.
(() => {
    const verdict = (state, reason) => ({ state, reason });
    if (location.origin !== 'https://chatgpt.com') return verdict(false, 'Page is not on chatgpt.com');
    const visible = node => {
      if (node.closest('[data-message-author-role], [data-content-search-unit-key], [data-chatgpt-search-unit-key], [data-testid^="conversation-turn"], article[data-turn], [data-turn-key]')) return false;
      const rect = node.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return false;
      for (let current = node; current; current = current.parentElement) {
        const style = window.getComputedStyle(current);
        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0' || current.getAttribute('aria-hidden') === 'true') return false;
      }
      return !node.disabled && node.getAttribute('aria-disabled') !== 'true';
    };
    const active = Array.from(document.querySelectorAll('main > header[data-app-shell-titlebar="true"] button[aria-label="Turn off temporary chat"]')).filter(visible);
    const inactive = Array.from(document.querySelectorAll('main > header[data-app-shell-titlebar="true"] button[aria-label="Turn on temporary chat"]')).filter(visible);
    if (inactive.length) return verdict(false, 'Header shows Temporary Chat turned off');
    if (active.length > 1) return verdict(false, 'Header shows more than one Temporary Chat control');
    // ChatGPT removes the toggle after dispatch. Its mounted conversation view
    // still carries the mode and server ID; require both on the current tree.
    const path = location.pathname.split('/').filter(Boolean);
    const conversationId = path.length === 2 && path[0] === 'c' ? path[1] : null;
    // Qualified public route trace: ChatGPT mounts this local client route
    // before the server ID. It never authorises payload entry or capture.
    const provisional = /^\/c\/local-chatgpt%3A[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(location.pathname);
    const viewId = provisional ? decodeURIComponent(conversationId) : conversationId;
    // The toggle can unmount before both the route and conversation view.
    // Absence is unknown; an explicit inactive control above still refuses.
    if (!conversationId) {
      if (path.length !== 0) return verdict(false, 'Page is neither the landing page nor a conversation');
      return active.length === 1 ? verdict(true, null) : verdict(null, 'Landing page shows no Temporary Chat control');
    }
    const seen = new Set();
    const matches = new Set();
    for (const node of document.querySelectorAll('main *')) {
      if (!visible(node)) continue;
      const key = Object.keys(node).find(key => key.startsWith('__reactFiber$'));
      let fiber = key ? node[key] : null;
      if (!fiber) continue;
      let root = fiber;
      while (root.return) root = root.return;
      if (root.stateNode?.current !== root) {
        fiber = fiber.alternate;
        if (!fiber) continue;
        root = fiber;
        while (root.return) root = root.return;
        if (root.stateNode?.current !== root) continue;
      }
      for (; fiber && !seen.has(fiber); fiber = fiber.return) {
        seen.add(fiber);
        const props = fiber.memoizedProps;
        // From 10 November 2026 ChatGPT keys the view by its local client ID and names the
        // server ID only in its history; the older layout named it in scrollStateConversationId.
        const boundId = props?.scrollStateConversationId ?? props?.history?.serverConversationId;
        if (boundId !== viewId || props.isChatConversation !== true) continue;
        if (props.isTemporaryChat !== true) return verdict(false, 'Conversation view is not marked Temporary Chat');
        if (props.isWorkConversation !== false) return verdict(false, 'Conversation view is not confirmed outside Work');
        if (props.isSharedConversation !== false) return verdict(false, 'Conversation view is not confirmed unshared');
        matches.add(fiber);
      }
    }
    // Responsive layouts can mount multiple views of this same conversation.
    // Every matching current view above must agree that it is temporary.
    // A conversation route can precede its mounted view. This is unknown,
    // never authority to enter payloads, dispatch, capture or retire a target.
    if (provisional) return verdict(null, 'Conversation has only its provisional local route');
    return matches.size > 0 ? verdict(true, null) : verdict(null, 'Temporary Chat binding not found for the conversation view');
  })()
