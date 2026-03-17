Based on {plan}, please create tests before implementing the production code.

**CRITICAL: Create tests covering ALL sections and ALL phases in the plan. Do not stop at Phase 1 — every phase and section listed in the plan must have corresponding tests.**

**What to do:**
1. Read the entire plan from start to finish and identify ALL phases and sections before writing any tests.
2. Examine the existing code and tests of the target module to grasp the testing patterns.
3. Create unit tests for the planned functionality.
4. Determine the necessity of integration tests and create them if required:
   - Is there a data flow spanning three or more modules?
   - Does a new status/state merge into an existing workflow?
   - Do new options propagate to the end through the call chain?
   - If any of these apply, create integration tests.
5. Run a build (type check) to ensure there are no syntax errors in the test code.
6. If necessary for the build (type check), you may modify the implementation files using features like `todo!()` in Rust, for example.

**Policy for creating tests:**
- Follow the project's existing test patterns (naming conventions, directory structure, helpers).
- Write tests using the Given-When-Then structure.
- One test, one concept. Do not mix multiple concerns into one test.
- Cover happy path, error cases, boundary values, and edge cases.
- Tests should be written with the assumption that they will pass once implementation is complete.
