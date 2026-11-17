// The decoder for a prompt ChatGPT rewrote as escaped Markdown. Statements, not a function: Rust
// splices them around owned_commit.js and submission-witness.js, so the message ChatGPT stores
// and the one it sends are held to the same rule. ChatGPT stores a message with an autolinked URL
// or link syntax as escaped Markdown, and its sent text differed from such a prompt in every run
// that held a web address (November 2026). This decodes only with link evidence and no unescaped
// Markdown syntax outside a restored URL, and returns null otherwise.
const markdownSourceText = value => {
    const punctuation = /[!-\/:-@\[-\x60{-~]/;
    const unescape = text => text.replace(/\\([!-\/:-@\[-\x60{-~])/g, '$1');
    const link = /\[(https?:(?:\\\S|[^\\\[\]\s])*)\]\((https?:(?:\\\S|[^\\()\[\]\s])*)\)/y;
    const autolinkEnd = index => {
      while (index < value.length) {
        if (/[\s)\]}"']/.test(value[index])) return true;
        if (value[index] === '\\' && punctuation.test(value[index + 1] ?? '')) index++;
        else if (/[.,;:!?]/.test(value[index])) index++;
        else return false;
      }
      return true;
    };
    const completeLiteralLink = text => /(^|[^!])\[[^\[\]\r\n]+\]\([^()\s]+\)/.test(text);
    const hasHost = text => { try { return /^https?:\/\//.test(text) && Boolean(new URL(text).host); } catch { return false; } };
    let text = '';
    let evidence = 0;
    let escapedLinkOpener = false;
    for (let index = 0; index < value.length;) {
      link.lastIndex = index;
      const match = value[index] === '[' ? link.exec(value) : null;
      if (match && unescape(match[1]) === unescape(match[2]) &&
          (index === 0 || /[\s([{\"']/.test(value[index - 1])) &&
          autolinkEnd(link.lastIndex) && hasHost(unescape(match[2]))) {
        text += unescape(match[2]);
        evidence++;
        index = link.lastIndex;
      } else if (value[index] === '\\' && punctuation.test(value[index + 1] ?? '')) {
        if (value[index + 1] === '(' && value[index - 1] === ']') escapedLinkOpener = true;
        text += value[index + 1];
        index += 2;
      } else if (/[\x60*<]/.test(value[index]) || (value[index] === ']' && value[index + 1] === '(')) {
        return null;
      } else text += value[index++];
    }
    return evidence || (escapedLinkOpener && completeLiteralLink(text)) ? text : null;
  };
