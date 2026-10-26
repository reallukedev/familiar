import { useState } from 'react';
import { IconButton } from '../../components/Button';
import { Confirm } from '../../components/Confirm';
import { Icon } from '../../components/Icon';
import { ActionMenu } from '../../components/Menu';
import { POWER_LABELS, powerMenu } from '../../lib/status';
import type { Instance } from '../../lib/types';
import { usePower, WARNS } from './usePower';
import { withUndo } from '../../lib/undo';

/** The toolbar's power button: every power action that fits the Mac's state. */
export function PowerMenu({ instance, power }: { instance: Instance; power: ReturnType<typeof usePower> }) {
  const actions = powerMenu(instance);
  return (
    <>
      <ActionMenu
        label={`Power options for ${instance.name}`}
        trigger={
          <IconButton label={`Power options for ${instance.name}`} disabled={!actions.length || !!power.busy}>
            <Icon name="power" size={19} weight={2} />
          </IconButton>
        }
        items={actions.map((action, index) => ({
          label: POWER_LABELS[action],
          onSelect: () => power.run(action),
          tone: action === 'force_stop' ? 'destructive' : 'default',
          separated: action === 'force_stop' && index > 0,
        }))}
      />
      <PowerConfirm instance={instance} power={power} />
    </>
  );
}

const WARNING = {
  stop: { title: 'Shut down', message: 'Apps on the Mac quit, so anything unsaved is lost.', action: 'Shut Down' },
  restart: { title: 'Restart', message: 'Apps on the Mac quit, so anything unsaved is lost.', action: 'Restart' },
  force_stop: { title: 'Force shut down', message: 'Turns the Mac off at once. Anything unsaved is lost, and your assistant’s current task stops midway.', action: 'Force Shut Down' },
} as const;

/** The warning before a power action that can lose work. Used wherever power actions run. */
export function PowerConfirm({ instance, power }: { instance: Instance; power: ReturnType<typeof usePower> }) {
  const action = power.confirming;
  const [shown, setShown] = useState<keyof typeof WARNING>('stop');
  if (action && action in WARNING && action !== shown) setShown(action as keyof typeof WARNING);
  const copy = WARNING[shown];
  return (
    <Confirm
      open={!!action && action in WARNING}
      onOpenChange={(open) => !open && power.setConfirming(null)}
      title={`${copy.title} ${instance.name}?`}
      message={copy.message}
      action={copy.action}
      remember={WARNS[shown]?.remember}
      onConfirm={() => withUndo(`${copy.action === 'Restart' ? 'Restarting' : 'Shutting down'} ${instance.name}…`, () => power.perform(shown))}
    />
  );
}
