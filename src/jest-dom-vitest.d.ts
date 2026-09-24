// Vitest 5 replaced its chai-based expect with its own `Assertion` interface,
// which the generic "@testing-library/jest-dom" types do not augment. Load the
// dedicated vitest augmentation project-wide so matcher types (toBeInTheDocument,
// toBeDisabled, ...) resolve in every typechecked test file — including when
// src/test-setup.ts is excluded from the tsconfig program.
import "@testing-library/jest-dom/vitest";
