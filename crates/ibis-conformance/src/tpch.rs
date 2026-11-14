//! Ibis's TPC-H queries (ibis/backends/tests/tpc/h/test_queries.py), ported
//! line for line. Keep them shaped like the Python so a diff against Ibis is
//! easy to read: `t.col("x")` is Ibis `t.x`; `add_date(...)` calls are
//! precomputed date literals (noted inline).

use ibis_core::prelude::*;
use ibis_core::{Result, Table};

use JoinKind::{Inner, Left};

pub const TABLES: [&str; 8] = ["customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier"];

pub struct Tpch {
    pub customer: Table,
    pub lineitem: Table,
    pub nation: Table,
    pub orders: Table,
    pub part: Table,
    pub partsupp: Table,
    pub region: Table,
    pub supplier: Table,
}

impl Tpch {
    pub fn load(b: &dyn Backend) -> Result<Tpch> {
        Ok(Tpch {
            customer: b.table("customer")?,
            lineitem: b.table("lineitem")?,
            nation: b.table("nation")?,
            orders: b.table("orders")?,
            part: b.table("part")?,
            partsupp: b.table("partsupp")?,
            region: b.table("region")?,
            supplier: b.table("supplier")?,
        })
    }
}

pub type Query = fn(&Tpch) -> Result<Table>;

pub const QUERIES: [(&str, Query); 22] = [
    ("h01", h01),
    ("h02", h02),
    ("h03", h03),
    ("h04", h04),
    ("h05", h05),
    ("h06", h06),
    ("h07", h07),
    ("h08", h08),
    ("h09", h09),
    ("h10", h10),
    ("h11", h11),
    ("h12", h12),
    ("h13", h13),
    ("h14", h14),
    ("h15", h15),
    ("h16", h16),
    ("h17", h17),
    ("h18", h18),
    ("h19", h19),
    ("h20", h20),
    ("h21", h21),
    ("h22", h22),
];

fn s(v: &str) -> Expr {
    lit(v)
}

pub fn h01(db: &Tpch) -> Result<Table> {
    let t = &db.lineitem;
    let q = t.filter([t.col("l_shipdate").le(date("1998-09-02"))])?; // add_date("1998-12-01", dd=-90)
    let discount_price = t.col("l_extendedprice") * (lit(1) - t.col("l_discount"));
    let charge = discount_price.clone() * (lit(1) + t.col("l_tax"));
    q.group_by(["l_returnflag", "l_linestatus"])
        .agg([
            t.col("l_quantity").sum().name("sum_qty"),
            t.col("l_extendedprice").sum().name("sum_base_price"),
            discount_price.sum().name("sum_disc_price"),
            charge.sum().name("sum_charge"),
            t.col("l_quantity").mean().name("avg_qty"),
            t.col("l_extendedprice").mean().name("avg_price"),
            t.col("l_discount").mean().name("avg_disc"),
            count().name("count_order"),
        ])?
        .order_by(["l_returnflag", "l_linestatus"])
}

pub fn h02(db: &Tpch) -> Result<Table> {
    let (part, supplier, partsupp, nation, region) = (&db.part, &db.supplier, &db.partsupp, &db.nation, &db.region);
    let expr = part
        .join(partsupp, [part.col("p_partkey").eq(partsupp.col("ps_partkey"))], Inner)?
        .join(supplier, [supplier.col("s_suppkey").eq(partsupp.col("ps_suppkey"))], Inner)?
        .join(nation, [supplier.col("s_nationkey").eq(nation.col("n_nationkey"))], Inner)?
        .join(region, [nation.col("n_regionkey").eq(region.col("r_regionkey"))], Inner)?;
    let subexpr = partsupp
        .join(supplier, [supplier.col("s_suppkey").eq(partsupp.col("ps_suppkey"))], Inner)?
        .join(nation, [supplier.col("s_nationkey").eq(nation.col("n_nationkey"))], Inner)?
        .join(region, [nation.col("n_regionkey").eq(region.col("r_regionkey"))], Inner)?;
    let subexpr =
        subexpr.filter([subexpr.col("r_name").eq(s("EUROPE")), expr.col("p_partkey").eq(subexpr.col("ps_partkey"))])?;
    let q = expr.filter([
        expr.col("p_size").eq(lit(15)),
        expr.col("p_type").like("%BRASS"),
        expr.col("r_name").eq(s("EUROPE")),
        expr.col("ps_supplycost").eq(subexpr.col("ps_supplycost").min()),
    ])?;
    let q = q.select(["s_acctbal", "s_name", "n_name", "p_partkey", "p_mfgr", "s_address", "s_phone", "s_comment"])?;
    q.order_by([desc("s_acctbal"), asc("n_name"), asc("s_name"), asc("p_partkey")]).map(|q| q.limit(100, 0))
}

pub fn h03(db: &Tpch) -> Result<Table> {
    let (customer, orders, lineitem) = (&db.customer, &db.orders, &db.lineitem);
    let d = date("1995-03-15");
    let q = customer.join(orders, [customer.col("c_custkey").eq(orders.col("o_custkey"))], Inner)?;
    let q = q.join(lineitem, [lineitem.col("l_orderkey").eq(orders.col("o_orderkey"))], Inner)?;
    let q = q.filter([
        q.col("c_mktsegment").eq(s("BUILDING")),
        q.col("o_orderdate").lt(d.clone()),
        q.col("l_shipdate").gt(d),
    ])?;
    let q = q
        .group_by([q.col("l_orderkey"), q.col("o_orderdate"), q.col("o_shippriority")])
        .agg([(q.col("l_extendedprice") * (lit(1) - q.col("l_discount"))).sum().name("revenue")])?
        .relocate(["revenue"], "l_orderkey")?;
    Ok(q.order_by([desc("revenue"), asc("o_orderdate")])?.limit(10, 0))
}

pub fn h04(db: &Tpch) -> Result<Table> {
    let (orders, lineitem) = (&db.orders, &db.lineitem);
    let cond = lineitem.col("l_orderkey").eq(orders.col("o_orderkey"))
        & lineitem.col("l_commitdate").lt(lineitem.col("l_receiptdate"));
    let q = orders.filter([
        cond.any(),
        orders.col("o_orderdate").ge(date("1993-07-01")),
        orders.col("o_orderdate").lt(date("1993-10-01")), // add_date(DATE, dm=3)
    ])?;
    q.group_by([orders.col("o_orderpriority")]).agg([count().name("order_count")])?.order_by(["o_orderpriority"])
}

pub fn h05(db: &Tpch) -> Result<Table> {
    let (customer, lineitem, orders, supplier, nation, region) =
        (&db.customer, &db.lineitem, &db.orders, &db.supplier, &db.nation, &db.region);
    let q = customer.join(orders, [customer.col("c_custkey").eq(orders.col("o_custkey"))], Inner)?;
    let q = q.join(lineitem, [lineitem.col("l_orderkey").eq(orders.col("o_orderkey"))], Inner)?;
    let q = q.join(supplier, [lineitem.col("l_suppkey").eq(supplier.col("s_suppkey"))], Inner)?;
    let q = q.join(
        nation,
        [customer.col("c_nationkey").eq(supplier.col("s_nationkey"))
            & supplier.col("s_nationkey").eq(nation.col("n_nationkey"))],
        Inner,
    )?;
    let q = q.join(region, [nation.col("n_regionkey").eq(region.col("r_regionkey"))], Inner)?;
    let q = q.filter([
        q.col("r_name").eq(s("ASIA")),
        q.col("o_orderdate").ge(date("1994-01-01")),
        q.col("o_orderdate").lt(date("1995-01-01")), // add_date(DATE, dy=1)
    ])?;
    let revexpr = q.col("l_extendedprice") * (lit(1) - q.col("l_discount"));
    q.group_by([q.col("n_name")]).agg([revexpr.sum().name("revenue")])?.order_by([desc("revenue")])
}

pub fn h06(db: &Tpch) -> Result<Table> {
    let q = &db.lineitem;
    let q = q.filter([
        q.col("l_shipdate").ge(date("1994-01-01")),
        q.col("l_shipdate").lt(date("1995-01-01")), // add_date(DATE, dy=1)
        q.col("l_discount").between(lit(0.05), lit(0.07)),
        q.col("l_quantity").lt(lit(24)),
    ])?;
    q.aggregate([(q.col("l_extendedprice") * q.col("l_discount")).sum().name("revenue")])
}

pub fn h07(db: &Tpch) -> Result<Table> {
    let (supplier, lineitem, orders, customer, nation) =
        (&db.supplier, &db.lineitem, &db.orders, &db.customer, &db.nation);
    let q = supplier.join(lineitem, [supplier.col("s_suppkey").eq(lineitem.col("l_suppkey"))], Inner)?;
    let q = q.join(orders, [orders.col("o_orderkey").eq(lineitem.col("l_orderkey"))], Inner)?;
    let q = q.join(customer, [customer.col("c_custkey").eq(orders.col("o_custkey"))], Inner)?;
    let n1 = nation;
    let n2 = nation.view();
    let q = q.join(n1, [supplier.col("s_nationkey").eq(n1.col("n_nationkey"))], Inner)?;
    let q = q.join(&n2, [customer.col("c_nationkey").eq(n2.col("n_nationkey"))], Inner)?;
    let q = q.select([
        n1.col("n_name").name("supp_nation"),
        n2.col("n_name").name("cust_nation"),
        lineitem.col("l_shipdate"),
        lineitem.col("l_extendedprice"),
        lineitem.col("l_discount"),
        lineitem.col("l_shipdate").year().name("l_year"),
        (lineitem.col("l_extendedprice") * (lit(1) - lineitem.col("l_discount"))).name("volume"),
    ])?;
    let q = q.filter([
        (q.col("cust_nation").eq(s("FRANCE")) & q.col("supp_nation").eq(s("GERMANY")))
            | (q.col("cust_nation").eq(s("GERMANY")) & q.col("supp_nation").eq(s("FRANCE"))),
        q.col("l_shipdate").between(date("1995-01-01"), date("1996-12-31")), // add_date(DATE, dy=2, dd=-1)
    ])?;
    q.group_by(["supp_nation", "cust_nation", "l_year"]).agg([q.col("volume").sum().name("revenue")])?.order_by([
        "supp_nation",
        "cust_nation",
        "l_year",
    ])
}

pub fn h08(db: &Tpch) -> Result<Table> {
    let (part, supplier, region, lineitem, orders, customer, nation) =
        (&db.part, &db.supplier, &db.region, &db.lineitem, &db.orders, &db.customer, &db.nation);
    let n1 = nation;
    let n2 = n1.view();
    let q = part.join(lineitem, [part.col("p_partkey").eq(lineitem.col("l_partkey"))], Inner)?;
    let q = q.join(supplier, [supplier.col("s_suppkey").eq(lineitem.col("l_suppkey"))], Inner)?;
    let q = q.join(orders, [lineitem.col("l_orderkey").eq(orders.col("o_orderkey"))], Inner)?;
    let q = q.join(customer, [orders.col("o_custkey").eq(customer.col("c_custkey"))], Inner)?;
    let q = q.join(n1, [customer.col("c_nationkey").eq(n1.col("n_nationkey"))], Inner)?;
    let q = q.join(region, [n1.col("n_regionkey").eq(region.col("r_regionkey"))], Inner)?;
    let q = q.join(&n2, [supplier.col("s_nationkey").eq(n2.col("n_nationkey"))], Inner)?;
    let q = q.select([
        orders.col("o_orderdate").year().name("o_year"),
        (lineitem.col("l_extendedprice") * (lit(1) - lineitem.col("l_discount"))).name("volume"),
        n2.col("n_name").name("nation"),
        region.col("r_name"),
        orders.col("o_orderdate"),
        part.col("p_type"),
    ])?;
    let q = q.filter([
        q.col("r_name").eq(s("AMERICA")),
        q.col("o_orderdate").between(date("1995-01-01"), date("1996-12-31")), // add_date(DATE, dy=2, dd=-1)
        q.col("p_type").eq(s("ECONOMY ANODIZED STEEL")),
    ])?;
    let q =
        q.mutate([cases([(q.col("nation").eq(s("BRAZIL")), q.col("volume"))], Some(lit(0))).name("nation_volume")])?;
    q.group_by([q.col("o_year")])
        .agg([(q.col("nation_volume").sum() / q.col("volume").sum()).name("mkt_share")])?
        .order_by(["o_year"])
}

pub fn h09(db: &Tpch) -> Result<Table> {
    let (part, supplier, lineitem, partsupp, orders, nation) =
        (&db.part, &db.supplier, &db.lineitem, &db.partsupp, &db.orders, &db.nation);
    let q = lineitem.join(supplier, [supplier.col("s_suppkey").eq(lineitem.col("l_suppkey"))], Inner)?;
    let q = q.join(
        partsupp,
        [partsupp.col("ps_suppkey").eq(lineitem.col("l_suppkey"))
            & partsupp.col("ps_partkey").eq(lineitem.col("l_partkey"))],
        Inner,
    )?;
    let q = q.join(part, [part.col("p_partkey").eq(lineitem.col("l_partkey"))], Inner)?;
    let q = q.join(orders, [orders.col("o_orderkey").eq(lineitem.col("l_orderkey"))], Inner)?;
    let q = q.join(nation, [supplier.col("s_nationkey").eq(nation.col("n_nationkey"))], Inner)?;
    let q = q.select([
        (q.col("l_extendedprice") * (lit(1) - q.col("l_discount")) - q.col("ps_supplycost") * q.col("l_quantity"))
            .name("amount"),
        q.col("o_orderdate").year().name("o_year"),
        q.col("n_name").name("nation"),
        q.col("p_name"),
    ])?;
    let q = q.filter([q.col("p_name").like("%green%")])?;
    q.group_by([q.col("nation"), q.col("o_year")])
        .agg([q.col("amount").sum().name("sum_profit")])?
        .order_by([asc("nation"), desc("o_year")])
}

pub fn h10(db: &Tpch) -> Result<Table> {
    let (customer, orders, lineitem, nation) = (&db.customer, &db.orders, &db.lineitem, &db.nation);
    let q = customer.join(orders, [customer.col("c_custkey").eq(orders.col("o_custkey"))], Inner)?;
    let q = q.join(lineitem, [lineitem.col("l_orderkey").eq(orders.col("o_orderkey"))], Inner)?;
    let q = q.join(nation, [customer.col("c_nationkey").eq(nation.col("n_nationkey"))], Inner)?;
    let q = q.filter([
        q.col("o_orderdate").ge(date("1993-10-01")) & q.col("o_orderdate").lt(date("1994-01-01")), // dm=3
        q.col("l_returnflag").eq(s("R")),
    ])?;
    let q = q
        .group_by([
            q.col("c_custkey"),
            q.col("c_name"),
            q.col("c_acctbal"),
            q.col("n_name"),
            q.col("c_address"),
            q.col("c_phone"),
            q.col("c_comment"),
        ])
        .agg([(q.col("l_extendedprice") * (lit(1) - q.col("l_discount"))).sum().name("revenue")])?
        .relocate(["revenue"], "c_name")?;
    Ok(q.order_by([desc("revenue")])?.limit(20, 0))
}

pub fn h11(db: &Tpch) -> Result<Table> {
    let (partsupp, supplier, nation) = (&db.partsupp, &db.supplier, &db.nation);
    let q = partsupp.join(supplier, [partsupp.col("ps_suppkey").eq(supplier.col("s_suppkey"))], Inner)?;
    let q = q.join(nation, [nation.col("n_nationkey").eq(supplier.col("s_nationkey"))], Inner)?;
    let q = q.filter([q.col("n_name").eq(s("GERMANY"))])?;
    let innerq = partsupp.join(supplier, [partsupp.col("ps_suppkey").eq(supplier.col("s_suppkey"))], Inner)?;
    let innerq = innerq.join(nation, [nation.col("n_nationkey").eq(supplier.col("s_nationkey"))], Inner)?;
    let innerq = innerq.filter([innerq.col("n_name").eq(s("GERMANY"))])?;
    let innerq = innerq.aggregate([(innerq.col("ps_supplycost") * innerq.col("ps_availqty")).sum().name("total")])?;
    let q =
        q.group_by([q.col("ps_partkey")]).agg([(q.col("ps_supplycost") * q.col("ps_availqty")).sum().name("value")])?;
    let q = q.filter([q.col("value").gt(innerq.col("total") * lit(0.0001))])?;
    q.order_by([desc("value")])
}

pub fn h12(db: &Tpch) -> Result<Table> {
    let (orders, lineitem) = (&db.orders, &db.lineitem);
    let q = orders.join(lineitem, [orders.col("o_orderkey").eq(lineitem.col("l_orderkey"))], Inner)?;
    let q = q.filter([
        q.col("l_shipmode").isin([s("MAIL"), s("SHIP")]),
        q.col("l_commitdate").lt(q.col("l_receiptdate")),
        q.col("l_shipdate").lt(q.col("l_commitdate")),
        q.col("l_receiptdate").ge(date("1994-01-01")),
        q.col("l_receiptdate").lt(date("1995-01-01")), // add_date(DATE, dy=1)
    ])?;
    q.group_by([q.col("l_shipmode")])
        .agg([
            q.col("o_orderpriority")
                .cases([(s("1-URGENT"), lit(1)), (s("2-HIGH"), lit(1))], Some(lit(0)))
                .sum()
                .name("high_line_count"),
            q.col("o_orderpriority")
                .cases([(s("1-URGENT"), lit(0)), (s("2-HIGH"), lit(0))], Some(lit(1)))
                .sum()
                .name("low_line_count"),
        ])?
        .order_by(["l_shipmode"])
}

pub fn h13(db: &Tpch) -> Result<Table> {
    let (customer, orders) = (&db.customer, &db.orders);
    let innerq = customer.join(
        orders,
        [customer.col("c_custkey").eq(orders.col("o_custkey")) & !orders.col("o_comment").like("%special%requests%")],
        Left,
    )?;
    let innerq = innerq.group_by([innerq.col("c_custkey")]).agg([innerq.col("o_orderkey").count().name("c_count")])?;
    innerq
        .group_by([innerq.col("c_count")])
        .agg([count().name("custdist")])?
        .order_by([desc("custdist"), desc("c_count")])
}

pub fn h14(db: &Tpch) -> Result<Table> {
    let (part, lineitem) = (&db.part, &db.lineitem);
    let q = lineitem.join(part, [lineitem.col("l_partkey").eq(part.col("p_partkey"))], Inner)?;
    let q = q.filter([
        q.col("l_shipdate").ge(date("1995-09-01")),
        q.col("l_shipdate").lt(date("1995-10-01")), // add_date(DATE, dm=1)
    ])?;
    let revenue = q.col("l_extendedprice") * (lit(1) - q.col("l_discount"));
    let promo_revenue = q.col("p_type").like("PROMO%").ifelse(revenue.clone(), lit(0));
    q.aggregate([(lit(100) * promo_revenue.sum() / revenue.sum()).name("promo_revenue")])
}

pub fn h15(db: &Tpch) -> Result<Table> {
    let (lineitem, supplier) = (&db.lineitem, &db.supplier);
    let qrev = lineitem.filter([
        lineitem.col("l_shipdate").ge(date("1996-01-01")),
        lineitem.col("l_shipdate").lt(date("1996-04-01")), // add_date(DATE, dm=3)
    ])?;
    let qrev = qrev
        .group_by([lineitem.col("l_suppkey")])
        .agg([(qrev.col("l_extendedprice") * (lit(1) - qrev.col("l_discount"))).sum().name("total_revenue")])?;
    let q = supplier.join(&qrev, [supplier.col("s_suppkey").eq(qrev.col("l_suppkey"))], Inner)?;
    let q = q.filter([q.col("total_revenue").eq(qrev.col("total_revenue").max())])?;
    let q = q.select(["s_suppkey", "s_name", "s_address", "s_phone", "total_revenue"])?;
    q.order_by(["s_suppkey"])
}

pub fn h16(db: &Tpch) -> Result<Table> {
    let (partsupp, part, supplier) = (&db.partsupp, &db.part, &db.supplier);
    let q = partsupp.join(part, [part.col("p_partkey").eq(partsupp.col("ps_partkey"))], Inner)?;
    let complaints = supplier.filter([supplier.col("s_comment").like("%Customer%Complaints%")])?;
    let q = q.filter([
        q.col("p_brand").ne(s("Brand#45")),
        !q.col("p_type").like("MEDIUM POLISHED%"),
        q.col("p_size").isin([49, 14, 23, 45, 19, 3, 36, 9].map(lit)),
        !q.col("ps_suppkey").isin_col(complaints.col("s_suppkey")),
    ])?;
    q.group_by([q.col("p_brand"), q.col("p_type"), q.col("p_size")])
        .agg([q.col("ps_suppkey").nunique().name("supplier_cnt")])?
        .order_by([desc("supplier_cnt"), asc("p_brand"), asc("p_type"), asc("p_size")])
}

pub fn h17(db: &Tpch) -> Result<Table> {
    let (lineitem, part) = (&db.lineitem, &db.part);
    let q = lineitem.join(part, [part.col("p_partkey").eq(col("l_partkey"))], Inner)?;
    let avg_qty = lineitem.filter([col("l_partkey").eq(q.col("p_partkey"))])?.col("l_quantity").mean();
    q.filter([
        col("p_brand").eq(s("Brand#23")),
        col("p_container").eq(s("MED BOX")),
        col("l_quantity").lt(lit(0.2) * avg_qty),
    ])?
    .aggregate([(col("l_extendedprice").sum() / lit(7.0)).name("avg_yearly")])
}

pub fn h18(db: &Tpch) -> Result<Table> {
    let (customer, orders, lineitem) = (&db.customer, &db.orders, &db.lineitem);
    let subq = lineitem
        .group_by([col("l_orderkey")])
        .agg([col("l_quantity").sum().name("qty_sum")])?
        .filter([col("qty_sum").gt(lit(300))])?;
    let q = customer
        .join(orders, [col("c_custkey").eq(orders.col("o_custkey"))], Inner)?
        .join(lineitem, [orders.col("o_orderkey").eq(lineitem.col("l_orderkey"))], Inner)?
        .filter([col("o_orderkey").isin_col(subq.col("l_orderkey"))])?;
    Ok(q.group_by(["c_name", "c_custkey", "o_orderkey", "o_orderdate", "o_totalprice"])
        .agg([col("l_quantity").sum().name("sum_qty")])?
        .order_by([desc("o_totalprice"), asc("o_orderdate")])?
        .limit(100, 0))
}

pub fn h19(db: &Tpch) -> Result<Table> {
    let (lineitem, part) = (&db.lineitem, &db.part);
    let q = lineitem.join(part, [part.col("p_partkey").eq(lineitem.col("l_partkey"))], Inner)?;
    let branch = |brand: &str, containers: [&str; 4], qty: i64, size: i64| {
        q.col("p_brand").eq(s(brand))
            & q.col("p_container").isin(containers.map(s))
            & q.col("l_quantity").ge(lit(qty))
            & q.col("l_quantity").le(lit(qty + 10))
            & q.col("p_size").between(lit(1), lit(size))
            & q.col("l_shipmode").isin([s("AIR"), s("AIR REG")])
            & q.col("l_shipinstruct").eq(s("DELIVER IN PERSON"))
    };
    let q1 = branch("Brand#12", ["SM CASE", "SM BOX", "SM PACK", "SM PKG"], 1, 5);
    let q2 = branch("Brand#23", ["MED BAG", "MED BOX", "MED PKG", "MED PACK"], 10, 10);
    let q3 = branch("Brand#34", ["LG CASE", "LG BOX", "LG PACK", "LG PKG"], 20, 15);
    let q = q.filter([q1 | q2 | q3])?;
    q.aggregate([(q.col("l_extendedprice") * (lit(1) - q.col("l_discount"))).sum().name("revenue")])
}

pub fn h20(db: &Tpch) -> Result<Table> {
    let (supplier, nation, partsupp, part, lineitem) = (&db.supplier, &db.nation, &db.partsupp, &db.part, &db.lineitem);
    let q1 = supplier.join(nation, [supplier.col("s_nationkey").eq(nation.col("n_nationkey"))], Inner)?;
    let q3 = part.filter([part.col("p_name").like("forest%")])?;
    let q2 = partsupp;
    let q4 = lineitem.filter([
        lineitem.col("l_partkey").eq(q2.col("ps_partkey")),
        lineitem.col("l_suppkey").eq(q2.col("ps_suppkey")),
        lineitem.col("l_shipdate").ge(date("1994-01-01")),
        lineitem.col("l_shipdate").lt(date("1995-01-01")), // add_date(DATE, dy=1)
    ])?;
    let q2 = q2.filter([
        partsupp.col("ps_partkey").isin_col(q3.col("p_partkey")),
        partsupp.col("ps_availqty").gt(lit(0.5) * q4.col("l_quantity").sum()),
    ])?;
    let q1 = q1.filter([q1.col("n_name").eq(s("CANADA")), q1.col("s_suppkey").isin_col(q2.col("ps_suppkey"))])?;
    let q1 = q1.select(["s_name", "s_address"])?;
    q1.order_by(["s_name"])
}

pub fn h21(db: &Tpch) -> Result<Table> {
    let (supplier, lineitem, orders, nation) = (&db.supplier, &db.lineitem, &db.orders, &db.nation);
    let l2 = lineitem.view();
    let l3 = lineitem.view();
    let q = supplier.join(lineitem, [supplier.col("s_suppkey").eq(lineitem.col("l_suppkey"))], Inner)?;
    let q = q.join(orders, [orders.col("o_orderkey").eq(lineitem.col("l_orderkey"))], Inner)?;
    let q = q.join(nation, [supplier.col("s_nationkey").eq(nation.col("n_nationkey"))], Inner)?;
    let q = q.select([
        q.col("l_orderkey").name("l1_orderkey"),
        q.col("o_orderstatus"),
        q.col("l_receiptdate"),
        q.col("l_commitdate"),
        q.col("l_suppkey").name("l1_suppkey"),
        q.col("s_name"),
        q.col("n_name"),
    ])?;
    let q = q.filter([
        q.col("o_orderstatus").eq(s("F")),
        q.col("l_receiptdate").gt(q.col("l_commitdate")),
        q.col("n_name").eq(s("SAUDI ARABIA")),
        (l2.col("l_orderkey").eq(q.col("l1_orderkey")) & l2.col("l_suppkey").ne(q.col("l1_suppkey"))).any(),
        !(l3.col("l_orderkey").eq(q.col("l1_orderkey"))
            & l3.col("l_suppkey").ne(q.col("l1_suppkey"))
            & l3.col("l_receiptdate").gt(l3.col("l_commitdate")))
        .any(),
    ])?;
    Ok(q.group_by([q.col("s_name")])
        .agg([count().name("numwait")])?
        .order_by([desc("numwait"), asc("s_name")])?
        .limit(100, 0))
}

pub fn h22(db: &Tpch) -> Result<Table> {
    let (customer, orders) = (&db.customer, &db.orders);
    let codes = ["13", "31", "23", "29", "30", "18", "17"].map(s);
    let cc = || col("c_phone").substr(lit(0), Some(lit(2)));
    let q =
        customer
            .filter([col("c_acctbal").gt(lit(0.0)), cc().isin(codes.clone())])?
            .aggregate([col("c_acctbal").mean().name("avg_bal")])?;
    let custsale = customer
        .filter([
            cc().isin(codes),
            col("c_acctbal").gt(q.col("avg_bal")),
            !orders.col("o_custkey").eq(customer.col("c_custkey")).any(),
        ])?
        .select([cc().name("cntrycode"), col("c_acctbal")])?;
    custsale
        .group_by(["cntrycode"])
        .agg([count().name("numcust"), col("c_acctbal").sum().name("totacctbal")])?
        .order_by(["cntrycode"])
}
