---
name: scope-request
description: Turn a vague request into a scoped, buildable brief. Investigate the code and the documents first, right-size the work, and ask only the questions whose answers change the outcome. Use before writing a ticket or starting work that is not trivial, or when a request such as "add X" or "fix the Y flow" does not say what is in scope, where it stops, or how success will be judged.
---

# scope-request

Good scoping is investigation first and questions second. Most questions a
request seems to raise are answered by the repository, and a person's
attention should go only to the real forks. The output is a short brief.

The conventions in [the skills README](../README.md) apply.

## Steps

1. **Investigate the code.** Find the real surfaces the request touches.
   Establish which components are involved (`manifest:components`), what
   already exists and what would be new, and how far a change would reach. For
   a sweep across many files, hand it to a subagent if you have one and keep
   only its conclusion. Check a claim against the tree before you rely on it:
   that a file is said to exist is not proof that it does.

2. **Investigate the documents.** Read what the repository has written down
   about intent and constraints, wherever its manifest says each lives:
   - the architecture (`manifest:docs.architecture`)
   - the stories this would serve (`manifest:docs.stories`) and how they map
     to code (`manifest:docs.mapping`)
   - the decisions already made in this area (`manifest:docs.decisions`)
   - what the product is for and what it has ruled out (`manifest:docs.goals`)

   A decision that is recorded is a constraint, not a suggestion. A request
   that contradicts one is a finding to raise, not a detail to work around.

3. **Right-size it.** One piece of work is one concern. Propose a split when
   the work would combine concerns that could ship separately, reach into
   several components that could be changed one at a time, or be too large
   for one person to review with care in one sitting. As a rule of thumb,
   that is more than about three hundred lines of production code. When it is
   too big, propose the smallest coherent first slice and list the rest as
   work to follow.

4. **Ask only what changes the outcome.** After investigating you will be
   left with a few real forks. Good candidates:
   - success criteria that could be read two ways
   - which story the work serves, when that is unclear or there is none
   - whether it is reachable by a user now, or built now and connected later
   - which edge cases are in scope and which are deferred
   - how each behaviour will be proven, especially where the obvious kind of
     test is the wrong tool

   Recommend a default with each. Do not ask what the repository already
   answers.

5. **Write the brief.** Short, and in this shape:
   - **Goal**
   - **Affected surfaces**, with file and line where known
   - **Story**, or "none" with why the work can still be validated
   - **In scope** and **Out of scope**
   - **Dependencies**
   - **How each behaviour will be proven**
   - **Decisions resolved**, and any still open

## Rules

- **Investigate before asking.** A question the tree already answers wastes a
  person's turn.
- **Never offer a bare menu.** Every question carries the answer you would
  choose and why.
- **Surface, do not bury.** If investigation contradicts the premise of the
  request, because the thing does not exist or already works, say so before
  scoping any further.
- **Scope lock.** A second concern found while scoping becomes its own piece
  of work to follow, not a larger brief.
