import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './app.css';
import { App } from './App';
import { installConnectionHooks } from './lib/connection';
import { installReauthHook } from './lib/reauth';
import { installRecoveryHook } from './lib/recovery';
import { installSessionHooks } from './lib/session';

async function boot() {
  // The demo backend exists only in development builds. In a production build
  // `import.meta.env.DEV` is false, so this branch and the demo module are
  // dropped from the bundle entirely.
  if (import.meta.env.DEV) {
    const params = new URLSearchParams(location.search);
    // ?demo&familiar: the mascot gallery, for reviewing its states.
    if (params.has('familiar')) {
      const { FamiliarGallery } = await import('./demo/FamiliarGallery');
      const root = document.getElementById('root');
      if (root) createRoot(root).render(<StrictMode><FamiliarGallery /></StrictMode>);
      return;
    }
    let remembered = false;
    try {
      remembered = sessionStorage.getItem('dot-home:demo') === '1';
    } catch {
      // Storage blocked: rely on the URL.
    }
    if (import.meta.env.VITE_DEMO === '1' || params.has('demo') || remembered) {
      const { installDemo } = await import('./demo/install');
      installDemo(params);
    }
  }
  installSessionHooks();
  installReauthHook();
  installRecoveryHook();
  installConnectionHooks();
  const root = document.getElementById('root');
  if (root) {
    createRoot(root).render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
  }
}

void boot();
