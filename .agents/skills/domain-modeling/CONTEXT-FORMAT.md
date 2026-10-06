# CONTEXT.md Format

## Structure

```md
# {Context Name}

{One or two sentence description of what this context is and why it exists.}

## Language

**Order**:
{A one or two sentence description of the term}
_Avoid_: Purchase, transaction

**Invoice**:
A request for payment sent to a customer after delivery.
_Avoid_: Bill, payment request

**Customer**:
A person or organization that places orders.
_Avoid_: Client, buyer, account
```

## Rules

- **Be opinionated.** Multiple words for one concept → pick best, list others under `_Avoid_`.
- **Tight definitions.** ≤2 sentences max. Define what it IS, not what it does.
- **Context-specific terms only.** General programming concepts (timeouts, error types, utility patterns) excluded even if used extensively — only concepts unique to this context belong.
- **Group under subheadings** when natural clusters emerge. Single cohesive area → flat list fine.

## Single vs multi-context repos

**Single context (most repos):** one `CONTEXT.md` at repo root.

**Multiple contexts:** `CONTEXT-MAP.md` at repo root lists contexts, their locations, relationships:

```md
# Context Map

## Contexts

- [Ordering](./src/ordering/CONTEXT.md): receives and tracks customer orders
- [Billing](./src/billing/CONTEXT.md): generates invoices and processes payments
- [Fulfillment](./src/fulfillment/CONTEXT.md): manages warehouse picking and shipping

## Relationships

- **Ordering → Fulfillment**: Ordering emits `OrderPlaced` events; Fulfillment consumes them to start picking
- **Fulfillment → Billing**: Fulfillment emits `ShipmentDispatched` events; Billing consumes them to generate invoices
- **Ordering ↔ Billing**: Shared types for `CustomerId` and `Money`
```

Skill infers structure:
- `CONTEXT-MAP.md` exists → read it to find contexts
- only root `CONTEXT.md` exists → single context
- neither exists → create root `CONTEXT.md` lazily when first term resolved

Multiple contexts → infer which one current topic relates to; unclear → ask.
