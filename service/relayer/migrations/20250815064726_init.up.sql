create type account_mark as enum ('default', 'always_deny', 'always_approve');

create table account (
    account_id varchar(64) not null,
    allowance numeric(39, 0) not null,
    allowance_locked numeric(39, 0) not null default 0,
    pending_transaction_hash varchar(45) default null,
    pending_transaction_issued_at timestamptz default null,
    mark account_mark not null default 'default',
    created_at timestamptz not null default current_timestamp,
    updated_at timestamptz not null default current_timestamp,
    primary key (account_id)
);

create or replace function updated_at()
returns trigger as $$
begin
    NEW.updated_at = now();
    return NEW;
end;
$$ language 'plpgsql';

create trigger updated_at_trigger before update on account for each row execute procedure updated_at();

create table call (
    id uuid not null default gen_random_uuid(),
    account_id varchar(64) not null,
    transaction_hash varchar(45) unique not null,
    allowance_spent numeric(39, 0) not null,
    succeeded bool not null,
    created_at timestamptz not null default current_timestamp,
    primary key (id),
    foreign key (account_id) references account (account_id)
);

create index idx__call__account_id on call (account_id);
