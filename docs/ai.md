# Ask OpenLens: the AI chat

`/p/<project>/ask` (**Ask** in the menu) lets anyone with access to a project ask questions about it in their own words. The model investigates with read-only tools and answers with the numbers behind each claim and links to the pages they come from. It can also **propose** a goal or a flag change, but nothing changes until an admin clicks *Apply this change*. Ask is off until an admin configures a model.

## Turning it on

Use **Settings → AI** (admins), or set environment variables. Environment variables win over the page.

| Setting | Environment variable | Notes |
|---|---|---|
| Provider | `OPENLENS_LLM_PROVIDER` | `anthropic` or `openai-compatible`. If only `OPENLENS_LLM_API_KEY` is set, the provider is Anthropic, as before. |
| Base URL | `OPENLENS_LLM_BASE_URL` | Required for `openai-compatible`, for example `http://litellm:4000/v1`. |
| Model | `OPENLENS_LLM_MODEL` | Defaults to `claude-opus-5-5` for Anthropic. |
| Fast model | `OPENLENS_LLM_FAST_MODEL` | Optional. Answers short lookups; questions that investigate ("why", "compare") use the main model. |
| API key | `OPENLENS_LLM_API_KEY` | Optional for local servers. On the page, the key is stored encrypted and is never shown again. |
| Daily budget | `OPENLENS_LLM_DAILY_TOKENS` | Tokens per project per day. Default 400,000. Each person may also ask 30 questions an hour. |

*Test* on the settings page sends one tiny request and reports the answer, the model and the time it took.

### Recipes

- **Anthropic:** provider `anthropic`, your API key. The stable prefix (rules, tools, project brief) is cached, so a follow-up question costs mostly its own tokens.
- **LiteLLM proxy** (routes to any provider, keys stay in the proxy): provider `openai-compatible`, base URL `http://litellm:4000/v1`, model as named in the proxy's config, key = a LiteLLM virtual key.
- **Ollama** (local: no data leaves your network): provider `openai-compatible`, base URL `http://ollama:11434/v1`, model such as `llama3.1` (it must support tool calls), no key.
- **OpenRouter, OpenAI, Azure OpenAI, Mistral, vLLM:** provider `openai-compatible`, with their base URL, model and key.

## What the model sees

- **Every request:** fixed rules, the tool list, and a project brief rebuilt daily and whenever the map changes. The brief holds the feature map (keys, names, areas, success events), the headline KPIs and the latest releases, the top insights, and the goals.
- **Your question,** plus the previous six questions and answers as plain text (no tool results).
- **Tool results,** as compact tables of at most 20 rows. Available tools: KPIs, insights, one feature's context, error groups, goals, and read-only SQL.
- **Never:** event properties, page URLs, replays, emails, user or session ids. SQL runs on an aggregate-only view: people are counted with `count(DISTINCT user_key)` (a keyed hash), never listed, and groups of fewer than 5 people are left out (D-021). An admin can allow person-level detail for one project on the Ask page.
- **What is sent** (on the Ask page, admins only) shows the exact body of the last request for the project. The key travels in a header and is never stored with it.

## Safety

- Tool results reach the model inside `<data>` blocks. The model is told that their content (event names, pages) is data from the app's users, never instructions.
- The model has no write tool. `propose_goal` and `propose_flag` only create a proposal. The server holds it under a single-use token, tied to the person and the project, which expires after 30 minutes. Only an admin's click on *Apply this change* applies it, with the same checks as the Goals and Flags pages.
- Every question and every applied change is in the audit log.
- Conversations are kept 30 days per person and project. *Export* gives them as Markdown, and *Forget this conversation* deletes them.

Card rewording on the board (D-016) still uses only `OPENLENS_LLM_API_KEY` with Anthropic.
