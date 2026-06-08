export type ToolRegistryItem = {
  id: string;
  label: string;
  category: "llm" | "search" | "web";
};

export const TOOL_REGISTRY: ToolRegistryItem[] = [
  { id: "llm.openai", label: "OpenAI", category: "llm" },
  { id: "llm.anthropic", label: "Anthropic", category: "llm" },
  { id: "llm.google", label: "Google", category: "llm" },
  { id: "search.brave", label: "Brave Search", category: "search" },
  { id: "search.tavily", label: "Tavily Search", category: "search" },
  { id: "web.extract", label: "Web Access", category: "web" }
];
