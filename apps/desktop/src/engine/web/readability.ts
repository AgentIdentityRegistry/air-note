function normalizeWhitespace(value: string): string {
  return value
    .replace(/\u00a0/g, " ")
    .replace(/\r\n/g, "\n")
    .replace(/[ \t]{2,}/g, " ")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function fallbackExtract(html: string): { title: string | null; text: string } {
  const titleMatch = html.match(/<title[^>]*>([\s\S]*?)<\/title>/i);
  const title = titleMatch?.[1] ? normalizeWhitespace(titleMatch[1]) : null;
  const text = normalizeWhitespace(
    html
      .replace(/<script[\s\S]*?<\/script>/gi, " ")
      .replace(/<style[\s\S]*?<\/style>/gi, " ")
      .replace(/<noscript[\s\S]*?<\/noscript>/gi, " ")
      .replace(/<template[\s\S]*?<\/template>/gi, " ")
      .replace(/<(br|\/p|\/div|\/li|\/h[1-6])\s*\/?>/gi, "\n")
      .replace(/<li\b[^>]*>/gi, "\n- ")
      .replace(/<[^>]+>/g, " ")
      .replace(/&nbsp;/gi, " ")
      .replace(/&amp;/gi, "&")
      .replace(/&lt;/gi, "<")
      .replace(/&gt;/gi, ">")
      .replace(/&quot;/gi, '"')
      .replace(/&#39;/gi, "'")
  );

  return { title, text };
}

function bestTextFromDocument(document: Document): string {
  const candidateSelectors = [
    "article",
    "main",
    "[role='main']",
    ".content",
    "#content",
    ".post",
    ".article"
  ];

  for (const selector of candidateSelectors) {
    const node = document.querySelector(selector);
    const text = normalizeWhitespace(node?.textContent ?? "");
    if (text.length > 120) {
      return text;
    }
  }

  const paragraphs = [...document.querySelectorAll("p")]
    .map((entry) => normalizeWhitespace(entry.textContent ?? ""))
    .filter((entry) => entry.length > 40);
  if (paragraphs.length) {
    return normalizeWhitespace(paragraphs.join("\n\n"));
  }

  return normalizeWhitespace(document.body?.textContent ?? "");
}

export function extractReadableDocument(html: string): { title: string | null; text: string } {
  if (!html.trim()) {
    return { title: null, text: "" };
  }

  if (typeof DOMParser === "undefined") {
    return fallbackExtract(html);
  }

  try {
    const parser = new DOMParser();
    const document = parser.parseFromString(html, "text/html");
    for (const selector of ["script", "style", "noscript", "template", "iframe"]) {
      document.querySelectorAll(selector).forEach((node) => node.remove());
    }

    const title = normalizeWhitespace(document.title || "") || null;
    const text = bestTextFromDocument(document);
    return { title, text };
  } catch {
    return fallbackExtract(html);
  }
}
