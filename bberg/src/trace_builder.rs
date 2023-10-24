use number::FieldElement;

use crate::circuit_builder::BBFiles;

pub trait TraceBuilder {
    fn create_trace_builder_cpp(
        &mut self,
        name: &str,
        fixed: &Vec<String>,
        witness: &Vec<String>,
        to_be_shifted: &Vec<String>,
    ) -> String;

    fn create_trace_builder_hpp(
        &mut self,
        name: &str,
        fixed: &Vec<String>,
        shifted: &Vec<String>,
    ) -> String;
}

fn trace_cpp_includes(relation_path: &str, name: &str) -> String {
    let boilerplate = r#"
#include "barretenberg/ecc/curves/bn254/fr.hpp"
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <sys/types.h>
#include <vector>
#include "barretenberg/proof_system/arithmetization/arithmetization.hpp"
"#
    .to_owned();

    format!(
        "
{boilerplate}
#include \"barretenberg/{relation_path}/{name}.hpp\"
#include \"barretenberg/proof_system/arithmetization/generated/{name}_arith.hpp\"
"
    )
}

fn trace_hpp_includes(name: &str) -> String {
    format!(
        "
    // AUTOGENERATED FILE
    #pragma once
    
    #include \"barretenberg/ecc/curves/bn254/fr.hpp\"
    #include \"barretenberg/proof_system/arithmetization/arithmetization.hpp\"
    #include \"barretenberg/proof_system/circuit_builder/circuit_builder_base.hpp\"
    
    #include \"./{name}_trace.cpp\"
    #include \"barretenberg/honk/flavor/generated/{name}_flavor.hpp\"
    #include \"barretenberg/proof_system/arithmetization/generated/{name}_arith.hpp\"
    #include \"barretenberg/proof_system/relations/generated/{name}.hpp\"
"
    )
}

fn build_shifts(fixed: &Vec<String>) -> String {
    let shift_assign: Vec<String> = fixed
        .iter()
        .map(|name| format!("row.{name}_shift = rows[(i) % rows.size()].{name};"))
        .collect();

    format!(
        "
    for (size_t i = 1; i < rows.size(); ++i) {{
        Row& row = rows[i-1];
        {}
        
    }}
    ",
        shift_assign.join("\n")
    )
}

fn build_empty_row(all_cols: &Vec<String>) -> String {
    // The empty row turns off all constraints when the ISLAST flag is set
    // We must check that this column exists, and return an error to the user if it is not found
    let is_last = all_cols.iter().find(|name| name.contains("ISLAST"));
    if is_last.is_none() {
        // TODO: make error
        panic!("ISLAST column not found in witness");
    }
    let is_last = is_last.unwrap();

    let initialize = all_cols
        .iter()
        .filter(|name| !name.contains("ISLAST")) // filter out islast
        .map(|name| format!("empty_row.{name} = fr(0);"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "
    auto empty_row = Row{{}};
    empty_row.{is_last} = fr(1);
    {initialize}
    rows.push_back(empty_row);

        
    "
    )
}

impl TraceBuilder for BBFiles {
    // Create trace builder
    // Generate some code that can read a commits.bin and constants.bin into data structures that bberg understands
    fn create_trace_builder_cpp(
        &mut self,
        name: &str,
        fixed: &Vec<String>,
        witness: &Vec<String>,
        to_be_shifted: &Vec<String>,
    ) -> String {
        // We are assuming that the order of the columns in the trace file is the same as the order in the witness file
        let includes = trace_cpp_includes(&self.rel, &name);
        let row_import = format!("using Row = {name}_vm::Row<barretenberg::fr>;");

        // NOTE: Both of these are also calculated elsewhere, this is extra work
        // TODO: Recalculated!
        let num_cols = fixed.len() + witness.len() * 2; // (2* as shifts)
        let fixed_name = fixed
            .iter()
            .map(|name| {
                let n = name.replace(".", "_");
                n.to_string()
            })
            .collect::<Vec<_>>();
        let witness_name = witness
            .iter()
            .map(|name| {
                let n = name.replace(".", "_");
                n.to_string()
            })
            .collect::<Vec<_>>();

        // TODO: remove the ol clones
        let all_names = [fixed_name.clone(), witness_name.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        // let empty_row = build_empty_row(&all_names);

        let compute_polys_assignemnt = all_names
            .iter()
            .map(|name| format!("polys.{name}[i] = rows[i].{name};",))
            .collect::<Vec<String>>()
            .join("\n");

        let all_poly_shifts = &witness_name
            .iter()
            .map(|name| format!("polys.{name}_shift = rows[i].{name}_shift;"))
            .collect::<Vec<String>>()
            .join("\n");

        let fixed_rows = fixed
            .iter()
            .map(|name| {
                let n = name.replace(".", "_");
                format!("current_row.{n} = read_field(constant_file);")
            })
            .collect::<Vec<_>>()
            .join("\n");

        let wit_rows = &witness_name
            .iter()
            .map(|n| {
                format!(
                    "
        current_row.{n} = read_field(commited_file);"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let construct_shifts = build_shifts(to_be_shifted);

        // NOTE: can we assume that the witness filename etc will stay the same?
        let read_from_file_boilerplate = format!(
            "
// AUTOGENERATED FILE
{includes}

using namespace barretenberg;

namespace proof_system {{

{row_import}
inline fr read_field(std::ifstream& file)
{{
    uint8_t buffer[32];
    file.read(reinterpret_cast<char*>(buffer), 32);

    // swap it to big endian ???? TODO: create utility
    for (int n = 0, m = 31; n < m; ++n, --m) {{
        std::swap(buffer[n], buffer[m]);
    }}

    return fr::serialize_from_buffer(buffer);
}}
    
inline std::vector<Row> read_both_file_into_cols(
    std::string const& commited_filename,
    std::string const& constants_filename
) {{
    std::vector<Row> rows;

    // open both files
    std::ifstream commited_file(commited_filename, std::ios::binary);
    if (!commited_file) {{
        std::cout << \"Error opening commited file\" << std::endl;
        return {{}};
    }}

    std::ifstream constant_file(constants_filename, std::ios::binary);
    if (!constant_file) {{
        std::cout << \"Error opening constant file\" << std::endl;
        return {{}};
    }}

    // We are assuming that the two files are the same length
    while (commited_file) {{
        Row current_row = {{}};

        {fixed_rows}
        {wit_rows}

        rows.push_back(current_row);
    }}

    // remove the last row - TODO: BUG!
    rows.pop_back();

    // Build out shifts from collected rows
    {construct_shifts}


    return rows;
}}

}}
    "
        );

        // TODO: remove return val, make traits for everything
        self.trace_hpp = Some(read_from_file_boilerplate.clone());
        read_from_file_boilerplate
    }

    fn create_trace_builder_hpp(
        &mut self,
        name: &str,
        all_cols: &Vec<String>,
        to_be_shifted: &Vec<String>,
    ) -> String {
        let includes = trace_hpp_includes(&name);

        let num_polys = all_cols.len();
        let num_cols = all_cols.len() + to_be_shifted.len();

        let compute_polys_assignemnt = all_cols
            .iter()
            .map(|name| format!("polys.{name}[i] = rows[i].{name};",))
            .collect::<Vec<String>>()
            .join("\n");

        let all_poly_shifts = &to_be_shifted
            .iter()
            .map(|name| format!("polys.{name}_shift = Polynomial(polys.{name}.shifted());"))
            .collect::<Vec<String>>()
            .join("\n");

        format!("
{includes}

using namespace barretenberg;

namespace proof_system {{

class {name}TraceBuilder {{
    public:
        using FF = arithmetization::{name}Arithmetization::FF;
        using Row = {name}_vm::Row<FF>;

        // TODO: tempalte
        using Polynomial = honk::flavor::{name}Flavor::Polynomial;
        using AllPolynomials = honk::flavor::{name}Flavor::AllPolynomials;

        static constexpr size_t num_fixed_columns = {num_cols};
        static constexpr size_t num_polys = {num_polys};
        std::vector<Row> rows;


        [[maybe_unused]] void build_circuit() {{
            rows = read_both_file_into_cols(\"../commits.bin\", \"../constants.bin\");
        }}


        AllPolynomials compute_polynomials() {{
            const auto num_rows = get_circuit_subgroup_size();
            AllPolynomials polys;

            // Allocate mem for each column
            for (size_t i = 0; i < num_fixed_columns; ++i) {{
                polys[i] = Polynomial(num_rows);
            }}

            for (size_t i = 0; i < rows.size(); i++) {{
                {compute_polys_assignemnt}
            }}

            {all_poly_shifts }

            return polys;
        }}

        [[maybe_unused]] bool check_circuit() {{
            // Get the rows from file
            build_circuit();

            auto polys = compute_polynomials();
            const size_t num_rows = polys[0].size();

            const auto evaluate_relation = [&]<typename Relation>(const std::string& relation_name) {{
                typename Relation::ArrayOfValuesOverSubrelations result;
                for (auto& r : result) {{
                    r = 0;
                }}
                constexpr size_t NUM_SUBRELATIONS = result.size();

                for (size_t i = 0; i < num_rows; ++i) {{
                    Relation::accumulate(result, polys.get_row(i), {{}}, 1);

                    bool x = true;
                    for (size_t j = 0; j < NUM_SUBRELATIONS; ++j) {{
                        if (result[j] != 0) {{
                            info(\"Relation \", relation_name, \", subrelation index \", j, \" failed at row \", i);
                            throw false;
                            x = false;
                        }}
                    }}
                    if (!x) {{
                        return false;
                    }}
                }}
                return true;
            }};

            return evaluate_relation.template operator()<{name}_vm::{name}<FF>>(\"{name}\");
        }}

        [[nodiscard]] size_t get_num_gates() const {{ return rows.size(); }}

        [[nodiscard]] size_t get_circuit_subgroup_size() const
        {{
            const size_t num_rows = get_num_gates();
            const auto num_rows_log2 = static_cast<size_t>(numeric::get_msb64(num_rows));
            size_t num_rows_pow2 = 1UL << (num_rows_log2 + (1UL << num_rows_log2 == num_rows ? 0 : 1));
            return num_rows_pow2;
        }}


}};
}}
        ")
    }
}
