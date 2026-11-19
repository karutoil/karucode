import type { JSX } from 'preact';
import { Suspense } from 'preact/compat';
import { ChatView } from '../components/ChatView';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { ProjectRail } from '../components/ProjectRail';
import { SessionsRail } from '../components/SessionsRail';
import { controlPlaneOpen } from '../state/controlPlane';
import { Skeleton } from '../ui';
import * as store from '../state/store';
import { lazyNamed } from '../lib/lazyView';

// Mission control is the heaviest screen in the app - three regions, its own
// snapshot store and a poll - and it lives behind one button. It travels as its
// own chunk so the workspace's first paint never waits for a screen the reader
// may not open.
const ControlPlaneLazy = lazyNamed(
  () => import('../components/control-plane/ControlPlane'),
  'ControlPlane',
);

export function WorkbenchLayout(): JSX.Element {
  return (
    /* One boundary per panel.
     *
     * The app has two layers at least: this outer shell boundary and one per
     * grid child or overlay. A malformed payload in the inspector, a plugin
     * panel that returns something its renderer did not expect, a theme file
     * with a corrupt token - each throws inside one component, and without a
     * boundary around it the throw unmounts the whole tree and leaves a white
     * page with no chat, no rails and no way back but the browser's reload.
     *
     * `ChatView` is deliberately NOT wrapped here: it carries its own
     * boundaries for the transcript and each of its sub-panels, so a crash in
     * the conversation does not also take the composer down.
     */
    <ErrorBoundary scope="The workspace" root>
      <div
        class={`app${store.narrowViewport.value ? ' is-narrow' : ''}`}
        data-rail-collapsed={store.railCollapsed.value ? 'true' : 'false'}
        data-inspector-collapsed={store.inspectorCollapsed.value ? 'true' : 'false'}
        data-inspector-mode={store.inspectorMode.value}
      >
        {controlPlaneOpen.value || (!store.narrowViewport.value && store.railCollapsed.value) ? (
          <ErrorBoundary scope="The project rail">
            <ProjectRail />
          </ErrorBoundary>
        ) : null}
        {controlPlaneOpen.value ? (
          <ErrorBoundary scope="Mission control">
            <Suspense
              fallback={
                // The fallback holds the grid cell the screen will take over, so
                // the rail and the chat column do not reflow when the chunk lands.
                <div class="flex-1 min-w-0 p-6" aria-busy="true">
                  <Skeleton label="Loading mission control" class="h-32 w-full" />
                </div>
              }
            >
              <ControlPlaneLazy />
            </Suspense>
          </ErrorBoundary>
        ) : (
          <>
            <ErrorBoundary scope="The sessions rail">
              <SessionsRail />
            </ErrorBoundary>
            <ChatView />
          </>
        )}
      </div>
    </ErrorBoundary>
  );
}
