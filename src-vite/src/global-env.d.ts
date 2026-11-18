// Minimal import.meta.env declaration. Deliberately NOT vite/client: pulling
// the full vite/client types into this project poisoned unrelated typings
// (+160 errors in one typecheck run). Declare only what the app reads.
interface ImportMeta {
  readonly env: { readonly DEV?: boolean };
}
