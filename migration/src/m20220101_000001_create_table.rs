use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Newsletter::Table)
                    .if_not_exists()
                    .col(pk_auto(Newsletter::Id))
                    .col(string_uniq(Newsletter::Link))
                    .col(integer_null(Newsletter::MessageId))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Program::Table)
                    .if_not_exists()
                    .col(pk_auto(Program::Id))
                    .col(integer(Program::NewsletterId))
                    .col(string(Program::Title))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_program_newsletter")
                            .from(Program::Table, Program::NewsletterId)
                            .to(Newsletter::Table, Newsletter::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Entry::Table)
                    .if_not_exists()
                    .col(pk_auto(Entry::Id))
                    .col(integer(Entry::ProgramId))
                    .col(timestamp_with_time_zone(Entry::Date))
                    .col(string_null(Entry::Details))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entry_program")
                            .from(Entry::Table, Entry::ProgramId)
                            .to(Program::Table, Program::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Entry::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(Program::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(Newsletter::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Newsletter {
    Table,
    Id,
    Link,
    MessageId,
}

#[derive(DeriveIden)]
enum Program {
    Table,
    Id,
    NewsletterId,
    Title,
}

#[derive(DeriveIden)]
enum Entry {
    Table,
    Id,
    ProgramId,
    Date,
    Details,
}
