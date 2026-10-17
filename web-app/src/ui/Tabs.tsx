export interface TabDef<T extends string> {
  id: T
  label: string
}

/** Segmented tab strip; scrolls sideways when it does not fit. */
export function Tabs<T extends string>({
  tabs,
  active,
  onChange,
}: {
  tabs: TabDef<T>[]
  active: T
  onChange: (id: T) => void
}) {
  return (
    <div className="no-scrollbar flex max-w-full gap-1 overflow-x-auto rounded-xl bg-surface2 p-1 sm:inline-flex" role="tablist">
      {tabs.map((t) => (
        <button
          key={t.id}
          role="tab"
          aria-selected={active === t.id}
          onClick={() => onChange(t.id)}
          className={`whitespace-nowrap rounded-lg px-3.5 py-1.5 text-[13px] font-semibold transition-colors ${
            active === t.id
              ? 'bg-raised text-ink shadow-card'
              : 'text-ink2 hover:text-ink'
          }`}
        >
          {t.label}
        </button>
      ))}
    </div>
  )
}
