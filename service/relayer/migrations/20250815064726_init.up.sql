drop table if exists allowance;

create table allowance (
    account_id varchar(64) not null,
    allowance numeric(39, 0) not null,
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

create trigger updated_at_trigger before update on allowance for each row execute procedure updated_at();

create table call (
    id uuid not null default gen_random_uuid(),
    account_id varchar(64) not null,
    receiver_id varchar(64) not null,
    method_name varchar(255) not null,
    args jsonb not null,
    transaction_id bytea unique not null,
    allowance_spent numeric(39, 0) not null,
    created_at timestamptz not null default current_timestamp,
    primary key (id)
);
