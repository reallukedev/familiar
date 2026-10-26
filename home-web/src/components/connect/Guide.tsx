import { Collapsible } from '@base-ui/react/collapsible';
import { Fragment, type ReactNode } from 'react';
import type { Client, GuideContext, StepBlock } from '../../lib/clients';
import { Button } from '../Button';
import { CopyField } from '../Copy';
import { FormError } from '../Field';
import { Icon } from '../Icon';
import { CodeBlock } from './CodeBlock';
import styles from './Guide.module.css';

type Props = {
  client: Client;
  context: GuideContext;
  /** Makes this client's token. Resolves once it exists (or failed, with `error` set). */
  onCreateToken: () => void;
  creating: boolean;
  error: string | null;
};

/** Step-by-step setup for one assistant, built from its guide in lib/clients. */
export function Guide({ client, context, onCreateToken, creating, error }: Props) {
  const guide = client.guide(context);
  return (
    <div className={styles.guide}>
      <p className="t-subhead secondary pretty">{rich(guide.intro)}</p>
      {guide.needs ? <p className={`t-footnote secondary pretty ${styles.needs}`}>{rich(guide.needs)}</p> : null}
      <ol className={styles.steps}>
        {guide.steps.map((step, index) => (
          <li key={index}>
            <div className={styles.step}>
              <span className="pretty">{rich(step.text)}</span>
              {step.block ? <Block block={step.block} client={client} context={context} onCreateToken={onCreateToken} creating={creating} error={error} /> : null}
            </div>
          </li>
        ))}
      </ol>
      <div className={styles.check}>
        <Icon name="check" size={15} weight={2.4} />
        <p className="t-subhead pretty">{rich(guide.check)}</p>
      </div>
      <Collapsible.Root className={styles.trouble}>
        <Collapsible.Trigger className={styles.toggle}>
          <Icon name="chevron-right" size={13} weight={2.4} className={styles.chevron} />
          If it doesn’t work
        </Collapsible.Trigger>
        <Collapsible.Panel className={styles.panel}>
          <dl className={styles.problems}>
            {guide.trouble.map((item) => (
              <div key={item.title} className={styles.problem}>
                <dt className="t-subhead">{item.title}</dt>
                <dd className="t-footnote secondary pretty">{rich(item.body)}</dd>
              </div>
            ))}
          </dl>
        </Collapsible.Panel>
      </Collapsible.Root>
    </div>
  );
}

function Block({ block, client, context, onCreateToken, creating, error }: { block: StepBlock; client: Client } & Omit<Props, 'client'>) {
  switch (block.type) {
    case 'url':
      return <CopyField value={context.url} what="server address" />;
    case 'name':
      return <CopyField value={block.value} what="name" />;
    case 'token':
      return context.token ? (
        <div className={styles.token}>
          <CopyField value={context.token} what="token" />
          <p className="t-footnote secondary">Familiar shows it only now. It’s already in the steps below.</p>
        </div>
      ) : (
        <div className={styles.token}>
          <span>
            <Button variant="tinted" busy={creating} onClick={onCreateToken}>
              Create Token for {client.name}
            </Button>
          </span>
          <FormError>{error}</FormError>
        </div>
      );
    case 'snippet':
      return (
        <CodeBlock
          code={block.snippet.code}
          label={block.snippet.label}
          what={block.snippet.what}
          secret={context.token}
          pending={block.snippet.usesToken && !context.token ? 'Create the token first' : undefined}
        />
      );
  }
}

/** `**bold**` for menu items and labels, `` `code` `` for things to type. */
function rich(text: string): ReactNode {
  return text.split(/(\*\*[^*]+\*\*|`[^`]+`)/).map((part, index) => {
    if (part.startsWith('**') && part.endsWith('**')) return <strong key={index}>{part.slice(2, -2)}</strong>;
    if (part.startsWith('`') && part.endsWith('`')) return <code key={index} translate="no">{part.slice(1, -1)}</code>;
    return <Fragment key={index}>{part}</Fragment>;
  });
}
