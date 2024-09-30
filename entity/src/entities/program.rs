//! `SeaORM` Entity, @generated by sea-orm-codegen 1.0.1

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "program")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub newsletter_id: i32,
    pub title: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::entry::Entity")]
    Entry,
    #[sea_orm(
        belongs_to = "super::newsletter::Entity",
        from = "Column::NewsletterId",
        to = "super::newsletter::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Newsletter,
}

impl Related<super::entry::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Entry.def()
    }
}

impl Related<super::newsletter::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Newsletter.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}