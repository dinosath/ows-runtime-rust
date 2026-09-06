Feature: for task

  Scenario: for loop accumulates across iterations
    Given a workflow with definition:
      """
      document:
        dsl: '1.0.3'
        namespace: default
        name: for
        version: '1.0.0'
      do:
        - loop:
            for:
              each: color
              in: '.colors'
            do:
              - mark:
                  set:
                    processed: '${ { colors: (.processed.colors + [ $color ]), indexes: (.processed.indexes + [ $index ]) } }'
      """
    And given the workflow input is:
      """
      colors:
        - red
        - green
        - blue
      """
    When the workflow is executed
    Then the workflow should complete with output:
      """
      processed:
        colors:
          - red
          - green
          - blue
        indexes:
          - 0
          - 1
          - 2
      """
