// Ported from the reference implementation's DOM expressions.
(cfg) => { // NOSONAR: the file is one function expression, which Rust calls through CDP
/* alcuin-dom:ui-warnings */

return (() => {
        const warningPattern = /too many requests|sending too many requests|too quickly|temporarily limited access|please wait a few minutes|rate limit|rate limited|\blimit|slow down|try again later|temporarily unavailable|something went wrong|failed to generate|no response from chatgpt|check your connection|verify you are human|unusual activity|cloudflare|challenge|captcha|login required|sign in/i;
        // Conversation text is never a notice: a user message, the answer roots of an assistant
        // message (all of it when none is marked) and the composer. An element inside one, or
        // wrapping one, reads it; a notice beside the answer, in the assistant's turn or not, and
        // a dialog styled like an answer are still read.
        const user = '[data-message-author-role="user"], [data-content-search-unit-key$=":user"], [data-chatgpt-search-unit-key$=":user"]';
        const reply = '[data-message-author-role="assistant"], [data-content-search-unit-key$=":assistant"], [data-chatgpt-search-unit-key$=":assistant"]';
        const answer = '[data-markdown-text-style="assistant-message"], .markdown, [data-message-content], .prose, [class*="MarkdownRoot"]';
        const conversation = Array.from(document.querySelectorAll(`${user}, ${reply}, #prompt-textarea`))
          .flatMap((node) => {
            const roots = node.matches(reply) ? Array.from(node.querySelectorAll(answer)) : [];
            return roots.length ? roots : [node];
          });
        const readsConversation = (element) => conversation.some((node) => node.contains(element) || element.contains(node));
        const selectors = [
          '[role="alert"]',
          '[role="status"]',
          '[role="dialog"]',
          '[aria-live]',
          '[data-testid*="toast" i]',
          '[data-testid*="banner" i]',
          '[data-testid*="error" i]',
          '[class*="toast" i]',
          '[class*="banner" i]'
        ];
        const isVisible = (element) => {
          if (!(element instanceof HTMLElement)) return false;
          let current = element;
          while (current) {
            const currentStyle = window.getComputedStyle(current);
            if (
              !currentStyle ||
              currentStyle.display === 'none' ||
              currentStyle.visibility === 'hidden' ||
              currentStyle.visibility === 'collapse' ||
              Number.parseFloat(currentStyle.opacity || '1') === 0
            ) {
              return false;
            }
            current = current.parentElement;
          }
          const rect = element.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        };
        const describe = (element, source, selector = null) => ({
          text: (element.innerText || element.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 1000),
          source,
          selector,
          role: element.getAttribute('role'),
          ariaLive: element.getAttribute('aria-live')
        });
        const out = [];
        const seen = new Set();
        const warningContainers = [];
        const overlapsWarningContainer = (element) => warningContainers.some((container) => (
          container !== element && (container.contains(element) || element.contains(container))
        ));
        const add = (element, entry) => {
          if (!entry.text || !warningPattern.test(entry.text.replace(/[_-]+/g, ' '))) return;
          const key = entry.text + '|' + (entry.role || '') + '|' + (entry.ariaLive || '');
          if (seen.has(key)) return;
          seen.add(key);
          warningContainers.push(element);
          out.push(entry);
        };
        for (const selector of selectors) {
          if (out.length >= 5) break;
          let elements = [];
          try {
            elements = Array.from(document.querySelectorAll(selector));
          } catch {
            elements = [];
          }
          for (const element of elements) {
            if (out.length >= 5) break;
            if (overlapsWarningContainer(element) || readsConversation(element)) continue;
            if (isVisible(element)) add(element, describe(element, 'selector', selector));
          }
        }
        return out.slice(0, 5);
      })();
}
